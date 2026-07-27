# Chart Studio A2 · PR1 — Registry, Shell & Cartesian Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce params v2, the render-boundary adapter, the viz registry and the studio shell units, and move the six cartesian viz types onto a descriptor — with the other thirteen still running on today's inline code behind a bridge.

**Architecture:** A `VizDescriptor` owns one viz family's config: reading it from stored params, writing it back as flat namespaced settings, turning it into a `QuerySpec`, listing missing required fields, and rendering its controls. `ChartExplore` looks a descriptor up in `vizRegistry` and stops knowing anything about cartesian charts. Stored params gain a v2 shape (`{version, spec, settings}`); everything that renders keeps reading today's v1 shape through `toRenderParams()`, so `ChartRenderer` and dashboards are untouched.

**Tech Stack:** React 18, TypeScript, Vite, TanStack Query, shadcn/ui, Vitest + Testing Library. Design tokens are OKLCH (use `var(--token)` in inline styles, never `hsl(var(--token))`).

**Spec:** `docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md`. This plan implements PR 1 of the three-PR delivery in that spec.

**Starting point:** branch `feat/chart-studio-a2` off `main` (already created, holds the spec commit). `cd frontend && npx tsc --noEmit` is clean and `npx vitest run` is green (1268 tests / 225 files).

**Ground rules for this PR:**

- Do **not** edit `ChartRenderer.tsx`, `echarts/**`, `PivotTable`, `ChartTable`, `MapChart`. If one of them needs a change, `toRenderParams` is wrong — fix the adapter instead.
- Do **not** touch the twelve non-cartesian branches in `ChartExplore` except where a step says so. They keep working through the bridge and move in PR2/PR3.
- No SQL migration in this PR. The backfill is PR3.

---

### Task 1: Params v2 types

The stored shape gains a version-2 variant. The v1 interface keeps its name because it stays the *render* shape that `toRenderParams` produces.

**Files:**
- Modify: `frontend/src/api/charts.ts` (add after the existing `ChartParams` interface, ~line 165)
- Test: `frontend/src/components/chart/viz/__tests__/params.test.ts` (create)

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/viz/__tests__/params.test.ts
import { describe, it, expect } from "vitest";
import { isParamsV2, EMPTY_SPEC } from "@/api/charts";
import type { AnyChartParams } from "@/api/charts";

describe("isParamsV2", () => {
  it("recognises a v2 params object", () => {
    const p: AnyChartParams = { version: 2, spec: EMPTY_SPEC, settings: { "cartesian.xAxis": "region" } };
    expect(isParamsV2(p)).toBe(true);
  });

  it("treats a legacy params object as v1", () => {
    const p: AnyChartParams = { spec: EMPTY_SPEC, cartesian: { xAxis: "region", yMetrics: ["total"] } };
    expect(isParamsV2(p)).toBe(false);
  });

  it("treats an empty object as v1 rather than throwing", () => {
    expect(isParamsV2({} as AnyChartParams)).toBe(false);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/params.test.ts`
Expected: FAIL — `isParamsV2` / `EMPTY_SPEC` are not exported from `@/api/charts`.

- [ ] **Step 3: Write the implementation**

Add to `frontend/src/api/charts.ts`, directly below the existing `ChartParams` interface:

```ts
/** Flat, namespaced viz settings — the v2 storage shape (see the A2 spec). */
export type VizSettings = Record<string, unknown>;

/** Params as stored from A2 onwards: the query, plus flat namespaced settings. */
export interface ChartParamsV2 {
  version: 2;
  spec: QuerySpec;
  settings: VizSettings;
}

/** What a stored chart may hold: pre-A2 rows are v1, new writes are v2. */
export type AnyChartParams = ChartParams | ChartParamsV2;

export const isParamsV2 = (p: AnyChartParams | undefined | null): p is ChartParamsV2 =>
  !!p && (p as ChartParamsV2).version === 2;

/** A spec that selects nothing — used as the starting point for a new chart. */
export const EMPTY_SPEC: QuerySpec = {
  metrics: [], dimensions: [], columns: [], filters: [], orderby: [],
  row_limit: 1000, rollup_groups: [],
};
```

Then widen the stored type on the `Chart` interface in the same file:

```ts
  params: AnyChartParams;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/params.test.ts && npx tsc --noEmit`
Expected: 3 tests PASS. `tsc` will now report errors at the places that read `chart.params` as v1 — that is expected and Task 3 fixes them. If `tsc` reports errors anywhere other than `ChartExplore.tsx`, `DashboardCardView.tsx`, `CardVizPanel.tsx` or `dashboard/params.ts`, stop and re-read this step.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/charts.ts frontend/src/components/chart/viz/__tests__/params.test.ts
git commit -m "feat(charts): params v2 types (version + flat namespaced settings)"
```

---

### Task 2: Cartesian codec

The cartesian family's config, read from either stored shape and written back as `cartesian.*` keys. Extracted as pure functions so the descriptor in Task 5 is assembly, not logic.

**Files:**
- Create: `frontend/src/components/chart/viz/cartesian-codec.ts`
- Test: `frontend/src/components/chart/viz/__tests__/cartesian-codec.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/viz/__tests__/cartesian-codec.test.ts
import { describe, it, expect } from "vitest";
import { cartesianFromParams, cartesianToSettings } from "../cartesian-codec";
import { EMPTY_SPEC } from "@/api/charts";
import type { CartesianConfig } from "@/api/charts";

const CFG: CartesianConfig = {
  xAxis: "region",
  yMetrics: ["total_revenue", "order_count"],
  series: "category",
  stacked: true,
  seriesColors: { total_revenue: "#123456" },
};

describe("cartesianToSettings", () => {
  it("writes only namespaced keys, and omits undefined ones", () => {
    const s = cartesianToSettings({ xAxis: "region", yMetrics: ["total_revenue"] });
    expect(s).toEqual({ "cartesian.xAxis": "region", "cartesian.yMetrics": ["total_revenue"] });
    expect(Object.keys(s).every((k) => k.startsWith("cartesian."))).toBe(true);
  });
});

describe("cartesianFromParams", () => {
  it("reads v2 settings", () => {
    const cfg = cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(CFG) });
    expect(cfg).toEqual(CFG);
  });

  it("reads a v1 params object from its nested cartesian key", () => {
    const cfg = cartesianFromParams({ spec: EMPTY_SPEC, cartesian: CFG });
    expect(cfg).toEqual(CFG);
  });

  it("falls back to the v1 spec metrics when a v1 chart has no cartesian key", () => {
    const cfg = cartesianFromParams({ spec: { ...EMPTY_SPEC, metrics: ["total_revenue"] } });
    expect(cfg).toEqual({ yMetrics: ["total_revenue"] });
  });

  it("returns an empty config for an empty v2 settings map", () => {
    const cfg = cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: {} });
    expect(cfg).toEqual({ yMetrics: [] });
  });

  it("round-trips config → settings → config", () => {
    expect(cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(CFG) })).toEqual(CFG);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/cartesian-codec.test.ts`
Expected: FAIL — cannot resolve `../cartesian-codec`.

- [ ] **Step 3: Write the implementation**

```ts
// frontend/src/components/chart/viz/cartesian-codec.ts
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, CartesianConfig, VizSettings } from "@/api/charts";

/** Settings namespace shared by every cartesian kind, so bar→line keeps its config. */
export const CARTESIAN_PREFIX = "cartesian.";

/** Config keys persisted for the cartesian family. Order is irrelevant; completeness is not. */
const KEYS = [
  "xAxis", "xMetric", "yMetrics", "series", "stacked", "horizontal", "smooth",
  "showLabels", "showLegend", "xLabel", "yLabel", "seriesTypes", "rightAxisMetrics",
  "logScale", "normalized", "goalValue", "goalLabel", "trendLine", "connectNulls",
  "step", "showStackTotal", "seriesColors", "seriesLabels", "yMin", "yMax",
  "showMarkers", "xScale",
] as const satisfies readonly (keyof CartesianConfig)[];

/** The `cartesian.*` slice of a v2 settings map. Undefined values are dropped, not stored as null. */
export function cartesianToSettings(cfg: CartesianConfig): VizSettings {
  const out: VizSettings = {};
  for (const k of KEYS) {
    const v = cfg[k];
    if (v !== undefined) out[CARTESIAN_PREFIX + k] = v;
  }
  return out;
}

export function cartesianFromSettings(settings: VizSettings): CartesianConfig {
  const out = { yMetrics: [] } as CartesianConfig;
  for (const k of KEYS) {
    const v = settings[CARTESIAN_PREFIX + k];
    if (v !== undefined) (out as Record<string, unknown>)[k] = v;
  }
  return out;
}

/**
 * Config from either stored shape. The v1 branch is the fallback chain that used to live
 * in ChartExplore's hydration effect: older charts predate the `cartesian` key and only
 * carry their metrics in the spec.
 */
export function cartesianFromParams(params: AnyChartParams): CartesianConfig {
  if (isParamsV2(params)) return cartesianFromSettings(params.settings);
  return params.cartesian ?? { yMetrics: params.spec?.metrics ?? [] };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/cartesian-codec.test.ts`
Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/cartesian-codec.ts frontend/src/components/chart/viz/__tests__/cartesian-codec.test.ts
git commit -m "feat(charts): cartesian settings codec (v2 flat keys, v1 fallback)"
```

---

### Task 3: `toRenderParams` adapter

Everything that renders keeps consuming today's nested shape. This is the single translation point, and the reason no renderer or dashboard file changes in A2.

**Files:**
- Create: `frontend/src/components/chart/viz/renderParams.ts`
- Test: `frontend/src/components/chart/viz/__tests__/renderParams.test.ts`
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx:73`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/viz/__tests__/renderParams.test.ts
import { describe, it, expect } from "vitest";
import { toRenderParams } from "../renderParams";
import { EMPTY_SPEC } from "@/api/charts";
import type { ChartParams } from "@/api/charts";

const SPEC = { ...EMPTY_SPEC, metrics: ["total_revenue"], dimensions: ["region"] };

describe("toRenderParams", () => {
  it("passes a v1 params object through untouched", () => {
    const v1: ChartParams = { spec: SPEC, cartesian: { xAxis: "region", yMetrics: ["total_revenue"] } };
    expect(toRenderParams("bar", v1)).toBe(v1);
  });

  it("rebuilds the nested cartesian key from v2 settings", () => {
    const out = toRenderParams("bar", {
      version: 2,
      spec: SPEC,
      settings: { "cartesian.xAxis": "region", "cartesian.yMetrics": ["total_revenue"], "cartesian.stacked": true },
    });
    expect(out.spec).toEqual(SPEC);
    expect(out.cartesian).toEqual({ xAxis: "region", yMetrics: ["total_revenue"], stacked: true });
  });

  it("carries the shared display keys across for any viz", () => {
    const out = toRenderParams("table", {
      version: 2,
      spec: SPEC,
      settings: {
        column_settings: { total_revenue: { number_style: "currency" } },
        conditional_formatting: [{ id: "r1" }],
      },
    });
    expect(out.columnSettings).toEqual({ total_revenue: { number_style: "currency" } });
    expect(out.conditionalFormatting).toEqual([{ id: "r1" }]);
  });

  it("omits the cartesian key for a non-cartesian viz", () => {
    const out = toRenderParams("pie", { version: 2, spec: SPEC, settings: { "cartesian.xAxis": "region" } });
    expect(out.cartesian).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/renderParams.test.ts`
Expected: FAIL — cannot resolve `../renderParams`.

- [ ] **Step 3: Write the implementation**

```ts
// frontend/src/components/chart/viz/renderParams.ts
import { isCartesianViz, isParamsV2 } from "@/api/charts";
import type { AnyChartParams, ChartParams, Rule, VizType } from "@/api/charts";
import type { ColumnFormat } from "@/components/chart/format-types";
import { cartesianFromSettings } from "./cartesian-codec";

/**
 * The shape every renderer still speaks. v1 params pass through; v2 params are
 * un-flattened here and nowhere else, so ChartRenderer, PivotTable, ChartTable and the
 * dashboard's `vizOverride` merge all keep working on the pre-A2 keys.
 *
 * Only the namespaces migrated so far are un-flattened — PR2 adds pivot/table/object/map
 * and PR3 the remaining nine. This function and its callers disappear in sub-project B.
 */
export function toRenderParams(viz: VizType, params: AnyChartParams): ChartParams {
  if (!isParamsV2(params)) return params;
  const s = params.settings;
  const out: ChartParams = {
    spec: params.spec,
    columnSettings: s.column_settings as Record<string, ColumnFormat> | undefined,
    conditionalFormatting: s.conditional_formatting as Rule[] | undefined,
  };
  if (isCartesianViz(viz)) out.cartesian = cartesianFromSettings(s);
  return out;
}
```

- [ ] **Step 4: Wire the dashboard card through the adapter**

In `frontend/src/components/chart/dashboard/DashboardCardView.tsx`, line 73 currently reads:

```tsx
const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
```

Replace it with:

```tsx
// The override is a v1-shaped partial stored in dashboard JSONB, so it is merged AFTER
// the adapter — that is why no stored override needs migrating for A2.
const eff = applyVizOverride(
  { ...chart, params: toRenderParams(chart.viz_type, chart.params) },
  card.options.vizOverride as Record<string, unknown> | undefined,
);
```

and add the import:

```tsx
import { toRenderParams } from "@/components/chart/viz/renderParams";
```

Then change `applyVizOverride`'s signature in `frontend/src/components/chart/dashboard/params.ts:364` so it takes the already-adapted params:

```ts
export function applyVizOverride(
  chart: { viz_type: VizType; params: ChartParams },
  override?: Record<string, unknown>,
): { viz_type: VizType; params: ChartParams } {
```

The body is unchanged.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd frontend && npx vitest run src/components/chart/viz src/components/chart/dashboard && npx tsc --noEmit`
Expected: the new file's 4 tests PASS, existing dashboard tests stay green, and `tsc` now reports errors only in `ChartExplore.tsx` (fixed in Task 7).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz/renderParams.ts frontend/src/components/chart/viz/__tests__/renderParams.test.ts frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/components/chart/dashboard/params.ts
git commit -m "feat(charts): toRenderParams adapter at the render boundary"
```

---

### Task 4: Descriptor interface and registry

The contract plus an empty registry. Kept separate from the cartesian module so the shell can import the registry without pulling every descriptor's JSX into a cycle.

**Files:**
- Create: `frontend/src/components/chart/viz/types.ts`
- Create: `frontend/src/components/chart/viz/registry.ts`
- Test: `frontend/src/components/chart/viz/__tests__/registry.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/viz/__tests__/registry.test.ts
import { describe, it, expect } from "vitest";
import { CARTESIAN_VIZ } from "@/api/charts";
import { getDescriptor, hasDescriptor } from "../registry";

describe("vizRegistry", () => {
  it("has a descriptor for every cartesian kind in PR1", () => {
    for (const k of CARTESIAN_VIZ) expect(hasDescriptor(k)).toBe(true);
  });

  it("reports no descriptor for a viz still on the bridge", () => {
    expect(hasDescriptor("pie")).toBe(false);
  });

  it("returns a descriptor whose key matches the lookup", () => {
    expect(getDescriptor("bar")?.key).toBe("bar");
  });

  it("gives each cartesian kind its own meta", () => {
    expect(getDescriptor("bar")?.meta.name).not.toBe(getDescriptor("line")?.meta.name);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/registry.test.ts`
Expected: FAIL — cannot resolve `../registry`.

- [ ] **Step 3: Write the types**

```ts
// frontend/src/components/chart/viz/types.ts
import type { FC, ReactNode } from "react";
import type { ChartFilter, AnyChartParams, QuerySpec, VizSettings, VizType } from "@/api/charts";
import type { ChartTypeMeta } from "@/components/chart/chartTypes";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

/** Query state every viz shares; descriptors fold it into the spec they build. */
export interface SharedQueryState {
  filters: ChartFilter[];
  rowLimit: number;
  temporalUnits: Record<string, string>;
}

/** What a control gets: its config, a patcher, the dataset, and the last result. */
export interface VizControlProps<Cfg> {
  config: Cfg;
  onChange: (patch: Partial<Cfg>) => void;
  /** Dataset columns and metrics — what the chart *can* be configured with. */
  columns: DatasetColumn[];
  metrics: DatasetMetric[];
  /**
   * Columns and rows of the last result. Controls that list series (per-series colours,
   * labels) derive their own list from these — the shell must not compute anything
   * viz-specific on their behalf.
   */
  resultColumns: { name: string; type: string }[];
  rows: unknown[][];
  /** Renders the shared "Date grouping" row for a temporal dimension, or null. */
  unitRow: (col?: string) => ReactNode;
}

export interface VizControl<Cfg> {
  id: string;
  section: "data" | "display";
  render: FC<VizControlProps<Cfg>>;
}

/**
 * Everything the studio shell needs to know about one visualization. The shell never
 * reads inside `Cfg`; only this descriptor does.
 */
export interface VizDescriptor<Cfg = unknown> {
  key: VizType;
  meta: ChartTypeMeta;
  /** Settings namespace, e.g. "cartesian." — this descriptor owns only these keys. */
  prefix: string;
  defaultConfig: Cfg;

  fromParams(params: AnyChartParams): Cfg;
  toSettings(cfg: Cfg): VizSettings;
  buildSpec(cfg: Cfg, shared: SharedQueryState): QuerySpec;
  requiredFields(cfg: Cfg): string[];

  controls: VizControl<Cfg>[];
}
```

`DatasetColumn` (`api/datasets.ts:48`) and `DatasetMetric` (`:53`) are the existing exports — no new types are needed for the dataset side.

- [ ] **Step 4: Write the registry**

```ts
// frontend/src/components/chart/viz/registry.ts
import { CARTESIAN_VIZ } from "@/api/charts";
import type { VizType } from "@/api/charts";
import { cartesianDescriptor } from "./cartesian";
import type { VizDescriptor } from "./types";

/**
 * Viz types that have moved onto a descriptor. Everything absent here still runs on
 * ChartExplore's inline code via the bridge; PR2 and PR3 empty that bridge out.
 */
const REGISTRY: Partial<Record<VizType, VizDescriptor<never>>> = Object.fromEntries(
  CARTESIAN_VIZ.map((k) => [k, cartesianDescriptor(k)]),
) as Partial<Record<VizType, VizDescriptor<never>>>;

export const getDescriptor = (viz: VizType) => REGISTRY[viz];
export const hasDescriptor = (viz: VizType) => REGISTRY[viz] !== undefined;
```

- [ ] **Step 5: Run test to verify it fails for the right reason**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/registry.test.ts`
Expected: FAIL — cannot resolve `./cartesian`. Task 5 creates it; do not stub it here.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz/types.ts frontend/src/components/chart/viz/registry.ts frontend/src/components/chart/viz/__tests__/registry.test.ts
git commit -m "feat(charts): VizDescriptor contract + registry"
```

---

### Task 5: Cartesian descriptor

Assembles the codec, the spec builder, the required-field labels and the controls into one module — the first viz family off the if-ladders.

**Files:**
- Create: `frontend/src/components/chart/viz/cartesian.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/cartesian.test.tsx`
- Read (source of the moved code): `frontend/src/routes/explore/ChartExplore.tsx:206-210` (spec), `:673-712` (Data controls), `frontend/src/components/chart/requiredFields.ts:24-27` (labels)

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/viz/__tests__/cartesian.test.tsx
import { describe, it, expect } from "vitest";
import { cartesianDescriptor } from "../cartesian";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };

describe("cartesianDescriptor", () => {
  it("names the missing fields of an empty config", () => {
    expect(cartesianDescriptor("bar").requiredFields({ yMetrics: [] })).toEqual(["X-axis", "a metric"]);
  });

  it("reports nothing missing once an axis and a metric are set", () => {
    expect(cartesianDescriptor("bar").requiredFields({ xAxis: "region", yMetrics: ["total_revenue"] })).toEqual([]);
  });

  it("builds a spec with the x-axis and series as dimensions", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], series: "category" },
      SHARED,
    );
    expect(spec.dimensions).toEqual(["region", "category"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
    expect(spec.row_limit).toBe(1000);
  });

  it("prunes temporal units down to the dimensions actually grouped", () => {
    const spec = cartesianDescriptor("line").buildSpec(
      { xAxis: "order_date", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.temporal_units).toEqual({ order_date: "month" });
    const other = cartesianDescriptor("line").buildSpec({ xAxis: "region", yMetrics: ["m"] }, SHARED);
    expect(other.temporal_units).toEqual({});
  });

  it("puts scatter's x-metric first in the metric list", () => {
    const spec = cartesianDescriptor("scatter").buildSpec(
      { xAxis: "region", xMetric: "order_count", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["order_count", "total_revenue"]);
  });

  it("ignores an x-metric for a non-scatter kind", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", xMetric: "order_count", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("round-trips a v1 params object into settings the descriptor reads back", () => {
    const d = cartesianDescriptor("bar");
    const cfg = d.fromParams({ spec: EMPTY_SPEC, cartesian: { xAxis: "region", yMetrics: ["m"], stacked: true } });
    const back = d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) });
    expect(back).toEqual(cfg);
  });

  it("exposes data controls before display controls", () => {
    const sections = cartesianDescriptor("bar").controls.map((c) => c.section);
    expect(sections.indexOf("data")).toBeLessThan(sections.indexOf("display"));
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/viz/__tests__/cartesian.test.tsx`
Expected: FAIL — cannot resolve `../cartesian`.

- [ ] **Step 3: Write the descriptor**

```tsx
// frontend/src/components/chart/viz/cartesian.tsx
import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FieldZone } from "@/components/chart/config/FieldZone";
import { CartesianDisplaySection } from "@/components/chart/settings/CartesianDisplaySection";
import { cartesianSeriesNames } from "@/components/chart/echarts/options/cartesian";
import { Label } from "@/components/ui/label";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import type { CartesianConfig, CartesianViz, QuerySpec } from "@/api/charts";
import { cartesianFromParams, cartesianToSettings, CARTESIAN_PREFIX } from "./cartesian-codec";
import type { SharedQueryState, VizControl, VizControlProps, VizDescriptor } from "./types";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

function XAxisControl({ config, onChange, columns, unitRow }: VizControlProps<CartesianConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>X-axis</Label>
        <Select value={config.xAxis ?? ""} onValueChange={(v) => onChange({ xAxis: v || undefined })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {unitRow(config.xAxis)}
    </>
  );
}

function XMetricControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>X metric (optional)</Label>
      <Select value={config.xMetric ?? "__none"} onValueChange={(v) => onChange({ xMetric: v === "__none" ? undefined : v })}>
        <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None — use the dimension</SelectItem>
          {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  );
}

function YAxisControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  return (
    <FieldZone label="Y-axis" selected={config.yMetrics} onChange={(v) => onChange({ yMetrics: v })}
      options={metrics.map((m) => m.metric_name)} />
  );
}

function SeriesControl({ config, onChange, columns }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Series</Label>
      <Select value={config.series ?? "__none"} onValueChange={(v) => onChange({ series: v === "__none" ? undefined : v })}>
        <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None</SelectItem>
          {columns.filter((c) => c.column_name !== config.xAxis)
            .map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
        </SelectContent>
      </Select>
      {config.series && config.yMetrics.length > 1 && (
        <p className="text-xs text-muted-foreground">With a series, only the first metric is plotted.</p>
      )}
    </div>
  );
}

export function cartesianDescriptor(kind: CartesianViz): VizDescriptor<CartesianConfig> {
  const controls: VizControl<CartesianConfig>[] = [
    { id: "x", section: "data", render: XAxisControl },
    ...(kind === "scatter" ? [{ id: "xMetric", section: "data" as const, render: XMetricControl }] : []),
    { id: "y", section: "data", render: YAxisControl },
    { id: "series", section: "data", render: SeriesControl },
    {
      id: "display",
      section: "display",
      // The series list is derived here, not handed down: it is cartesian-specific, and
      // ChartSettingsSections.tsx:53 (which computes it today) goes away in PR3.
      render: ({ config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig>) => (
        <CartesianDisplaySection kind={kind} config={config} onChange={onChange}
          seriesNames={cartesianSeriesNames(resultColumns, rows, config)} />
      ),
    },
  ];

  return {
    key: kind,
    meta: CHART_TYPES.find((t) => t.key === kind)!,
    prefix: CARTESIAN_PREFIX,
    defaultConfig: { yMetrics: [] },
    fromParams: cartesianFromParams,
    toSettings: cartesianToSettings,
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.xAxis) out.push("X-axis");
      if (!cfg.yMetrics.length) out.push("a metric");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const dims = [cfg.xAxis, cfg.series].filter((d): d is string => !!d);
      const mets = kind === "scatter" && cfg.xMetric ? [cfg.xMetric, ...cfg.yMetrics] : cfg.yMetrics;
      return {
        metrics: mets, dimensions: dims, columns: [], filters: shared.filters, orderby: [],
        row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls,
  };
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd frontend && npx vitest run src/components/chart/viz`
Expected: all four viz test files PASS, including `registry.test.ts` from Task 4, which can now resolve `./cartesian`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/cartesian.tsx frontend/src/components/chart/viz/__tests__/cartesian.test.tsx
git commit -m "feat(charts): cartesian viz descriptor"
```

---

### Task 6: `useChartRun`

The run machinery currently spread across `ChartExplore` (`ranSpec`, `runNonce`, `lastRunSig`, `ranParams`, `ranTableDims`, `isStale`, `previewParams`, `useAutoRun`) becomes one hook, so the shell stops owning it.

**Files:**
- Create: `frontend/src/components/chart/useChartRun.ts`
- Test: `frontend/src/components/chart/__tests__/useChartRun.test.ts`
- Read (source of the moved logic): `frontend/src/routes/explore/ChartExplore.tsx:284-380`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/__tests__/useChartRun.test.ts
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useChartRun } from "../useChartRun";
import { EMPTY_SPEC } from "@/api/charts";

const SPEC_A = { ...EMPTY_SPEC, dimensions: ["region"], metrics: ["total_revenue"] };
const SPEC_B = { ...EMPTY_SPEC, dimensions: ["category"], metrics: ["total_revenue"] };

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useChartRun", () => {
  it("does not run while required fields are missing", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: true, buildSpec: () => SPEC_A, missing: ["a metric"] }),
    );
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current.ranSpec).toBeNull();
  });

  it("auto-runs after the debounce when the query signature changes", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: true, buildSpec: () => SPEC_A, missing: [] }),
    );
    act(() => { vi.advanceTimersByTime(600); });
    expect(result.current.ranSpec).toEqual(SPEC_A);
  });

  it("does not auto-run when auto is off, and runs on demand", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: false, buildSpec: () => SPEC_A, missing: [] }),
    );
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current.ranSpec).toBeNull();
    act(() => { result.current.run(); });
    expect(result.current.ranSpec).toEqual(SPEC_A);
  });

  it("reports the query as stale once the spec changes without a run", () => {
    const { result, rerender } = renderHook(
      ({ spec }) => useChartRun({ datasetId: "d1", autoRun: false, buildSpec: () => spec, missing: [] }),
      { initialProps: { spec: SPEC_A } },
    );
    act(() => { result.current.run(); });
    expect(result.current.isStale).toBe(false);
    rerender({ spec: SPEC_B });
    expect(result.current.isStale).toBe(true);
    expect(result.current.ranSpec).toEqual(SPEC_A);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/useChartRun.test.ts`
Expected: FAIL — cannot resolve `../useChartRun`.

- [ ] **Step 3: Write the hook**

```ts
// frontend/src/components/chart/useChartRun.ts
import { useRef, useState } from "react";
import { toast } from "sonner";
import type { QuerySpec } from "@/api/charts";
import { useAutoRun } from "./useAutoRun";

interface Args {
  datasetId: string;
  autoRun: boolean;
  /** The spec the current config would run. Called during render — keep it pure. */
  buildSpec: () => QuerySpec;
  /** Labels of the fields still missing; a non-empty list blocks running. */
  missing: string[];
}

/**
 * Owns "what has actually run": the spec in flight, the nonce that forces a refetch, and
 * the staleness comparison the preview uses to keep the previous result on screen (#345).
 * The shell passes config in and gets run state back; it holds none of this itself.
 */
export function useChartRun({ datasetId, autoRun, buildSpec, missing }: Args) {
  const [ranSpec, setRanSpec] = useState<QuerySpec | null>(null);
  const [runNonce, setRunNonce] = useState(0);
  const lastRunSig = useRef("");

  // Empty whenever the chart isn't runnable, so auto-run never fires an invalid query.
  const signature = datasetId && missing.length === 0 ? JSON.stringify({ datasetId, spec: buildSpec() }) : "";

  function run() {
    setRanSpec(null);
    if (!datasetId) { toast.error("Pick a dataset first"); return; }
    if (missing.length) { toast.error(`This chart needs ${missing.join(" and ")}`); return; }
    setRanSpec(buildSpec());
    setRunNonce((n) => n + 1);
    lastRunSig.current = signature;
  }

  useAutoRun({ enabled: autoRun, signature, onRun: run, lastRunRef: lastRunSig });

  return { ranSpec, runNonce, run, isStale: signature !== lastRunSig.current, reset: () => setRanSpec(null) };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/useChartRun.test.ts`
Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/useChartRun.ts frontend/src/components/chart/__tests__/useChartRun.test.ts
git commit -m "feat(charts): useChartRun hook owning run + auto-run + staleness"
```

---

### Task 7: Studio panel renders descriptor controls

The Data/Display tabs stop containing per-viz JSX and render whatever the active descriptor lists, grouped by section.

**Files:**
- Create: `frontend/src/components/chart/StudioPanel.tsx`
- Test: `frontend/src/components/chart/__tests__/StudioPanel.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/__tests__/StudioPanel.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StudioPanel } from "../StudioPanel";
import type { VizDescriptor } from "../viz/types";

const desc = {
  key: "bar",
  meta: { key: "bar", name: "Bar", icon: () => null, description: "" },
  prefix: "cartesian.",
  defaultConfig: { yMetrics: [] },
  fromParams: () => ({ yMetrics: [] }),
  toSettings: () => ({}),
  buildSpec: () => ({}),
  requiredFields: () => [],
  controls: [
    { id: "x", section: "data", render: () => <div>X-axis control</div> },
    { id: "d", section: "display", render: ({ onChange }) => <button onClick={() => onChange({ stacked: true })}>Stack</button> },
  ],
} as unknown as VizDescriptor<{ yMetrics: string[] }>;

const props = {
  descriptor: desc,
  config: { yMetrics: [] },
  onConfigChange: vi.fn(),
  columns: [], metrics: [], resultColumns: [], rows: [],
  unitRow: () => null,
  tab: "data" as const,
};

describe("StudioPanel", () => {
  it("renders the data-section controls on the Data tab", () => {
    render(<StudioPanel {...props} />);
    expect(screen.getByText("X-axis control")).toBeInTheDocument();
    expect(screen.queryByText("Stack")).not.toBeInTheDocument();
  });

  it("renders the display-section controls on the Display tab", () => {
    render(<StudioPanel {...props} tab="display" />);
    expect(screen.getByText("Stack")).toBeInTheDocument();
    expect(screen.queryByText("X-axis control")).not.toBeInTheDocument();
  });

  it("passes a control's patch up to onConfigChange", async () => {
    const onConfigChange = vi.fn();
    render(<StudioPanel {...props} tab="display" onConfigChange={onConfigChange} />);
    await userEvent.click(screen.getByRole("button", { name: "Stack" }));
    expect(onConfigChange).toHaveBeenCalledWith({ stacked: true });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/StudioPanel.test.tsx`
Expected: FAIL — cannot resolve `../StudioPanel`.

- [ ] **Step 3: Write the component**

```tsx
// frontend/src/components/chart/StudioPanel.tsx
import type { ReactNode } from "react";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";
import type { VizDescriptor } from "./viz/types";

interface Props<Cfg> {
  descriptor: VizDescriptor<Cfg>;
  config: Cfg;
  onConfigChange: (patch: Partial<Cfg>) => void;
  columns: DatasetColumn[];
  metrics: DatasetMetric[];
  resultColumns: { name: string; type: string }[];
  rows: unknown[][];
  unitRow: (col?: string) => ReactNode;
  tab: "data" | "display";
}

/**
 * Renders the active descriptor's controls for one tab. The panel knows nothing about
 * any specific viz — which is the whole point of the registry.
 */
export function StudioPanel<Cfg>({
  descriptor, config, onConfigChange, columns, metrics, resultColumns, rows, unitRow, tab,
}: Props<Cfg>) {
  const shown = descriptor.controls.filter((c) => c.section === tab);
  if (shown.length === 0) {
    return <p className="text-xs text-muted-foreground">No {tab} options for this chart type.</p>;
  }
  return (
    <>
      {shown.map(({ id, render: Control }) => (
        <Control key={id} config={config} onChange={onConfigChange}
          columns={columns} metrics={metrics} resultColumns={resultColumns} rows={rows} unitRow={unitRow} />
      ))}
    </>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/StudioPanel.test.tsx`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/StudioPanel.tsx frontend/src/components/chart/__tests__/StudioPanel.test.tsx
git commit -m "feat(charts): StudioPanel renders descriptor controls by section"
```

---

### Task 8: Wire the shell to the registry, bridge the rest

`ChartExplore` starts using the descriptor for cartesian charts and keeps its existing code for the other twelve types. This is the only task that edits `ChartExplore.tsx`, and it is a rewire, not a rewrite.

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Add the settings state and the descriptor lookup**

After the `const [cartesian, setCartesian] = useState<CartesianConfig>(...)` declaration, add:

```tsx
  const descriptor = getDescriptor(viz);
  // v2 settings for the migrated viz types. The bridge types keep their own useState
  // hooks until PR2/PR3 move them; `settings` is the shell's single opaque store.
  const [settings, setSettings] = useState<VizSettings>({});
```

with imports:

```tsx
import { getDescriptor } from "@/components/chart/viz/registry";
import { toRenderParams } from "@/components/chart/viz/renderParams";
import { StudioPanel } from "@/components/chart/StudioPanel";
import { isParamsV2 } from "@/api/charts";
import type { VizSettings } from "@/api/charts";
```

- [ ] **Step 2: Route the cartesian config through the descriptor**

`cartesian` stays the live config object for cartesian charts (the bridge types are untouched), but its reads and writes now go through the descriptor. Replace the `patchCartesian` helper at line 100 with:

```tsx
  const patchCartesian = (p: Partial<CartesianConfig>) => setCartesian((c) => ({ ...c, ...p }));
  // Config the active descriptor works with. Cartesian is the only migrated family in PR1.
  const descConfig = cartesian as never;
  const patchDescConfig = patchCartesian as (p: never) => void;
```

- [ ] **Step 3: Delegate spec building for cartesian**

In `specForRun()`, replace the cartesian branch (lines 206-210) with:

```tsx
    if (descriptor) {
      return descriptor.buildSpec(descConfig, { filters, rowLimit, temporalUnits });
    }
```

Leave every other branch exactly as it is.

- [ ] **Step 4: Delegate validation for cartesian**

In `validationInput()` / the `missingFields` call sites, compute the labels from the descriptor when there is one:

```tsx
  const missingNow = () =>
    descriptor ? descriptor.requiredFields(descConfig) : missingFields(viz, validationInput());
```

Then replace the two `missingFields(viz, validationInput())` calls (inside `run()` and in `querySignature`) with `missingNow()`, and the one inside the empty-state block with `datasetId ? missingNow() : ["a dataset"]`.

- [ ] **Step 5: Delegate save and hydration for cartesian**

In `paramsForSave()`, add as the first line of the function:

```tsx
    if (descriptor) {
      return {
        version: 2,
        spec: specForRun(),
        settings: {
          ...settings,
          ...descriptor.toSettings(descConfig),
          column_settings: columnSettings,
          conditional_formatting: conditionalFormatting,
        },
      };
    }
```

and change its return type to `AnyChartParams`.

In the hydration effect, replace the `isCartesianViz(c.viz_type)` branch with:

```tsx
    } else if (getDescriptor(c.viz_type)) {
      setCartesian(getDescriptor(c.viz_type)!.fromParams(p) as CartesianConfig);
      if (isParamsV2(p)) setSettings(p.settings);
```

and change the three lines below it that read `p.columnSettings` / `p.conditionalFormatting` to read through the adapter:

```tsx
    const rp = toRenderParams(c.viz_type, p);
    setColumnSettings(rp.columnSettings ?? {});
    setConditionalFormatting(rp.conditionalFormatting ?? []);
```

(`p.spec` is unchanged between v1 and v2, so the `rowLimit` / `temporalUnits` lines below keep working as they are.)

- [ ] **Step 6: Render the descriptor's controls**

In the Data tab, replace the whole `{isCartesianViz(viz) && ( … )}` block (lines 673-712) with:

```tsx
            {descriptor && (
              <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                unitRow={unitRow} tab="data" />
            )}
```

In the Display tab, wrap the existing `<ChartSettingsSections … />` so migrated types use the descriptor instead:

```tsx
            {descriptor ? (
              <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                unitRow={unitRow} tab="display" />
            ) : (
              <ChartSettingsSections vizType={viz} params={toRenderParams(viz, paramsForSave())}
                columns={resultColumns} rows={data.rows} onChange={applyParamsBack} />
            )}
```

`resultColumns` and `data.rows` already exist in this component (lines 292 and via `useSpecData`), so nothing new needs computing here — the cartesian display control derives its own series names.

- [ ] **Step 7: Send the preview through the adapter**

`previewParams()` currently returns the nested shape directly. Make it end with the adapter so the renderer keeps its v1 input:

```tsx
  const previewParams = (): ChartParams => {
    const base = isStale() && ranParams.current ? ranParams.current : paramsForSave();
    return { ...toRenderParams(viz, base), columnSettings: effCS };
  };
```

and change `ranParams` to `useRef<AnyChartParams | null>(null)`.

- [ ] **Step 8: Verify build and tests**

Run: `cd frontend && npx tsc --noEmit && npx eslint src/components/chart src/routes/explore/ChartExplore.tsx && npx vitest run`
Expected: `tsc` clean, eslint clean (the two pre-existing `maps/registry.ts` + `options/map.ts` warnings are fine), all tests PASS. If a test in `src/components/chart/settings/__tests__/ChartSettingsSections.test.tsx` fails, it is asserting on cartesian sections that now live in the descriptor — move those assertions to `viz/__tests__/cartesian.test.tsx` rather than deleting them.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): shell drives cartesian through the registry"
```

---

### Task 9: Full verify, deploy, live check, PR

**Files:** none (verification only)

- [ ] **Step 1: Full FE gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`
Expected: all four green; the test count should be at least 1268 plus the ~24 tests this plan adds.

- [ ] **Step 2: Deploy to kind**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

- [ ] **Step 3: Live check**

At `http://egret.localtest.me/explore/demo/charts/new?type=bar` on the Demo Sales dataset:

1. Pick the dataset, set X-axis `region` and metric `total_revenue` with Auto on — the chart runs by itself and draws four bars.
2. Open the Display tab — the cartesian display controls are all there and changing one (e.g. Stacked) re-renders without a new query.
3. Save as "A2 PR1 smoke", reload the saved URL — it renders on open. Confirm the stored shape is v2:
   `curl -s http://egret.localtest.me/api/workspaces/demo/charts | jq '.[] | select(.name=="A2 PR1 smoke") | .params | {version, keys: (.settings | keys)}'`
   Expected: `version: 2` and keys beginning `cartesian.`.
4. Open **`A1 studio smoke`** (saved in v1 before this change) — it must render identically, proving the v1 fallback path.
5. Open a dashboard containing a chart card, including one with a per-card viz override if present — unchanged rendering.
6. Open a `pie` chart and a `pivot_table` chart — both still work through the bridge.

- [ ] **Step 4: Open the PR (do not merge; await approval)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-studio-a2
gh pr create --base main --title "feat(charts): A2 PR1 — viz registry, studio shell, cartesian descriptor" --body "Implements PR 1 of docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md: params v2 + toRenderParams adapter + registry + StudioPanel/useChartRun + the cartesian family. The other 13 viz types still run on the inline bridge (PR2/PR3). No renderer or dashboard behaviour changes; no SQL migration yet."
```

---

## Notes for the implementer

- The bridge is deliberate, not laziness: `getDescriptor(viz)` returning `undefined` is how a viz type says "still inline". Never add a stub descriptor to make a type look migrated.
- `ChartSettingsSections` stays in the tree through PR3. Do not delete it here — the twelve bridge types still render their display controls with it.
- If `tsc` pushes you toward editing `ChartRenderer` or anything under `echarts/`, stop: the adapter is missing a key. Fix `toRenderParams`.
- Saved v1 charts are the highest-risk path in this PR. When in doubt, test against `A1 studio smoke` (a real v1 row on the kind cluster) rather than a hand-built fixture.
- Do not merge after opening the PR — this program awaits explicit approval per the user's standing instruction.
