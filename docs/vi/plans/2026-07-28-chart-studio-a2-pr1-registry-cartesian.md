# Chart Studio A2 · PR1 — Kế hoạch triển khai Registry, Shell & Cartesian

> **Cho worker tự động:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai plan này theo từng task. Các step dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Bản canonical (tiếng Anh):** `docs/en/plans/2026-07-28-chart-studio-a2-pr1-registry-cartesian.md` — code trong hai bản giống hệt nhau; bản này dịch phần diễn giải.

**Mục tiêu:** Đưa vào params v2, adapter ở biên render, viz registry và các đơn vị của studio shell, đồng thời chuyển sáu loại viz cartesian sang descriptor — mười ba loại còn lại vẫn chạy bằng code inline hiện tại sau một bridge.

**Kiến trúc:** Một `VizDescriptor` sở hữu cấu hình của một họ viz: đọc nó từ params đã lưu, ghi lại thành settings phẳng có namespace, biến nó thành `QuerySpec`, liệt kê các field bắt buộc còn thiếu, và vẽ control của nó. `ChartExplore` tra descriptor trong `vizRegistry` và không còn biết gì về chart cartesian. Params lưu trữ có thêm shape v2 (`{version, spec, settings}`); mọi thứ liên quan tới render vẫn đọc shape v1 hiện tại thông qua `toRenderParams()`, nên `ChartRenderer` và dashboard không bị đụng.

**Tech stack:** React 18, TypeScript, Vite, TanStack Query, shadcn/ui, Vitest + Testing Library. Design token là OKLCH (dùng `var(--token)` trong inline style, không dùng `hsl(var(--token))`).

**Spec:** `docs/vi/specs/2026-07-28-chart-studio-a2-viz-registry-design.md`. Plan này hiện thực PR 1 trong ba PR mà spec mô tả.

**Điểm xuất phát:** branch `feat/chart-studio-a2` tách từ `main` (đã tạo, đang giữ commit spec). `cd frontend && npx tsc --noEmit` sạch và `npx vitest run` xanh (1268 test / 225 file).

**Luật cho PR này:**

- **Không** sửa `ChartRenderer.tsx`, `echarts/**`, `PivotTable`, `ChartTable`, `MapChart`. Nếu một trong số đó cần sửa thì `toRenderParams` sai — sửa adapter.
- **Không** đụng mười hai nhánh non-cartesian trong `ChartExplore` trừ chỗ nào step nói rõ. Chúng vẫn chạy qua bridge và sẽ chuyển ở PR2/PR3.
- Không có migration SQL trong PR này. Backfill là việc của PR3.

---

### Task 1: Type params v2

Shape lưu trữ có thêm biến thể version 2. Interface v1 giữ nguyên tên vì nó vẫn là shape *render* mà `toRenderParams` sinh ra.

**Files:**
- Sửa: `frontend/src/api/charts.ts` (thêm sau interface `ChartParams` hiện có, ~dòng 165)
- Test: `frontend/src/components/chart/viz/__tests__/params.test.ts` (tạo mới)

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/params.test.ts`
Mong đợi: FAIL — `@/api/charts` chưa export `isParamsV2` / `EMPTY_SPEC`.

- [ ] **Step 3: Viết phần hiện thực**

Thêm vào `frontend/src/api/charts.ts`, ngay dưới interface `ChartParams` hiện có:

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

Rồi nới rộng kiểu lưu trữ trên interface `Chart` trong cùng file:

```ts
  params: AnyChartParams;
```

- [ ] **Step 4: Chạy test để xác nhận nó pass**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/params.test.ts && npx tsc --noEmit`
Mong đợi: 3 test PASS. `tsc` giờ sẽ báo lỗi ở những chỗ đọc `chart.params` như v1 — đó là dự kiến và Task 3 sẽ xử lý. Nếu `tsc` báo lỗi ở chỗ nào khác ngoài `ChartExplore.tsx`, `DashboardCardView.tsx`, `CardVizPanel.tsx` hay `dashboard/params.ts` thì dừng lại và đọc lại step này.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/charts.ts frontend/src/components/chart/viz/__tests__/params.test.ts
git commit -m "feat(charts): params v2 types (version + flat namespaced settings)"
```

---

### Task 2: Codec cartesian

Config của họ cartesian, đọc được từ cả hai shape lưu trữ và ghi lại thành các khoá `cartesian.*`. Tách thành hàm thuần để descriptor ở Task 5 chỉ là lắp ráp, không chứa logic.

**Files:**
- Tạo: `frontend/src/components/chart/viz/cartesian-codec.ts`
- Test: `frontend/src/components/chart/viz/__tests__/cartesian-codec.test.ts`

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/cartesian-codec.test.ts`
Mong đợi: FAIL — không resolve được `../cartesian-codec`.

- [ ] **Step 3: Viết phần hiện thực**

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

- [ ] **Step 4: Chạy test để xác nhận nó pass**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/cartesian-codec.test.ts`
Mong đợi: 6 test PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/cartesian-codec.ts frontend/src/components/chart/viz/__tests__/cartesian-codec.test.ts
git commit -m "feat(charts): cartesian settings codec (v2 flat keys, v1 fallback)"
```

---

### Task 3: Adapter `toRenderParams`

Mọi thứ liên quan tới render vẫn tiêu thụ shape lồng hiện tại. Đây là điểm dịch duy nhất, và là lý do không file renderer hay dashboard nào phải đổi trong A2.

**Files:**
- Tạo: `frontend/src/components/chart/viz/renderParams.ts`
- Test: `frontend/src/components/chart/viz/__tests__/renderParams.test.ts`
- Sửa: `frontend/src/components/chart/dashboard/DashboardCardView.tsx:73`

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/renderParams.test.ts`
Mong đợi: FAIL — không resolve được `../renderParams`.

- [ ] **Step 3: Viết phần hiện thực**

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

- [ ] **Step 4: Nối card dashboard qua adapter**

Trong `frontend/src/components/chart/dashboard/DashboardCardView.tsx`, dòng 73 hiện là:

```tsx
const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
```

Thay bằng:

```tsx
// The override is a v1-shaped partial stored in dashboard JSONB, so it is merged AFTER
// the adapter — that is why no stored override needs migrating for A2.
const eff = applyVizOverride(
  { ...chart, params: toRenderParams(chart.viz_type, chart.params) },
  card.options.vizOverride as Record<string, unknown> | undefined,
);
```

và thêm import:

```tsx
import { toRenderParams } from "@/components/chart/viz/renderParams";
```

Rồi đổi chữ ký của `applyVizOverride` trong `frontend/src/components/chart/dashboard/params.ts:364` để nó nhận params đã qua adapter:

```ts
export function applyVizOverride(
  chart: { viz_type: VizType; params: ChartParams },
  override?: Record<string, unknown>,
): { viz_type: VizType; params: ChartParams } {
```

Thân hàm giữ nguyên.

- [ ] **Step 5: Chạy test để xác nhận pass**

Chạy: `cd frontend && npx vitest run src/components/chart/viz src/components/chart/dashboard && npx tsc --noEmit`
Mong đợi: 4 test của file mới PASS, test dashboard hiện có vẫn xanh, và `tsc` giờ chỉ còn báo lỗi trong `ChartExplore.tsx` (Task 8 xử lý).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz/renderParams.ts frontend/src/components/chart/viz/__tests__/renderParams.test.ts frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/components/chart/dashboard/params.ts
git commit -m "feat(charts): toRenderParams adapter at the render boundary"
```

---

### Task 4: Interface descriptor và registry

Bản hợp đồng cộng một registry rỗng. Tách khỏi module cartesian để shell import được registry mà không kéo JSX của từng descriptor vào vòng lặp phụ thuộc.

**Files:**
- Tạo: `frontend/src/components/chart/viz/types.ts`
- Tạo: `frontend/src/components/chart/viz/registry.ts`
- Test: `frontend/src/components/chart/viz/__tests__/registry.test.ts`

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/registry.test.ts`
Mong đợi: FAIL — không resolve được `../registry`.

- [ ] **Step 3: Viết type**

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

`DatasetColumn` (`api/datasets.ts:48`) và `DatasetMetric` (`:53`) là export sẵn có — phía dataset không cần thêm type mới nào.

- [ ] **Step 4: Viết registry**

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

- [ ] **Step 5: Chạy test để xác nhận nó fail đúng lý do**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/registry.test.ts`
Mong đợi: FAIL — không resolve được `./cartesian`. Task 5 sẽ tạo file đó; **không** stub nó ở đây.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz/types.ts frontend/src/components/chart/viz/registry.ts frontend/src/components/chart/viz/__tests__/registry.test.ts
git commit -m "feat(charts): VizDescriptor contract + registry"
```

---

### Task 5: Descriptor cartesian

Lắp codec, hàm dựng spec, nhãn field bắt buộc và các control thành một module — họ viz đầu tiên thoát khỏi if-ladder.

**Files:**
- Tạo: `frontend/src/components/chart/viz/cartesian.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/cartesian.test.tsx`
- Đọc (nguồn của code được chuyển): `frontend/src/routes/explore/ChartExplore.tsx:206-210` (spec), `:673-712` (control tab Data), `frontend/src/components/chart/requiredFields.ts:24-27` (nhãn)

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/viz/__tests__/cartesian.test.tsx`
Mong đợi: FAIL — không resolve được `../cartesian`.

- [ ] **Step 3: Viết descriptor**

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

- [ ] **Step 4: Chạy test để xác nhận pass**

Chạy: `cd frontend && npx vitest run src/components/chart/viz`
Mong đợi: cả bốn file test trong `viz` PASS, gồm cả `registry.test.ts` của Task 4 — giờ đã resolve được `./cartesian`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/cartesian.tsx frontend/src/components/chart/viz/__tests__/cartesian.test.tsx
git commit -m "feat(charts): cartesian viz descriptor"
```

---

### Task 6: `useChartRun`

Bộ máy chạy hiện đang rải khắp `ChartExplore` (`ranSpec`, `runNonce`, `lastRunSig`, `ranParams`, `ranTableDims`, `isStale`, `previewParams`, `useAutoRun`) gom thành một hook, để shell thôi sở hữu nó.

**Files:**
- Tạo: `frontend/src/components/chart/useChartRun.ts`
- Test: `frontend/src/components/chart/__tests__/useChartRun.test.ts`
- Đọc (nguồn của logic được chuyển): `frontend/src/routes/explore/ChartExplore.tsx:284-380`

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/__tests__/useChartRun.test.ts`
Mong đợi: FAIL — không resolve được `../useChartRun`.

- [ ] **Step 3: Viết hook**

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

- [ ] **Step 4: Chạy test để xác nhận pass**

Chạy: `cd frontend && npx vitest run src/components/chart/__tests__/useChartRun.test.ts`
Mong đợi: 4 test PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/useChartRun.ts frontend/src/components/chart/__tests__/useChartRun.test.ts
git commit -m "feat(charts): useChartRun hook owning run + auto-run + staleness"
```

---

### Task 7: StudioPanel vẽ control của descriptor

Hai tab Data/Display thôi chứa JSX theo từng viz và chuyển sang vẽ đúng những gì descriptor đang hoạt động liệt kê, nhóm theo section.

**Files:**
- Tạo: `frontend/src/components/chart/StudioPanel.tsx`
- Test: `frontend/src/components/chart/__tests__/StudioPanel.test.tsx`

- [ ] **Step 1: Viết test cho fail**

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

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npx vitest run src/components/chart/__tests__/StudioPanel.test.tsx`
Mong đợi: FAIL — không resolve được `../StudioPanel`.

- [ ] **Step 3: Viết component**

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

- [ ] **Step 4: Chạy test để xác nhận pass**

Chạy: `cd frontend && npx vitest run src/components/chart/__tests__/StudioPanel.test.tsx`
Mong đợi: 3 test PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/StudioPanel.tsx frontend/src/components/chart/__tests__/StudioPanel.test.tsx
git commit -m "feat(charts): StudioPanel renders descriptor controls by section"
```

---

### Task 8: Nối shell vào registry, bridge phần còn lại

`ChartExplore` bắt đầu dùng descriptor cho chart cartesian và giữ nguyên code hiện có cho mười hai loại kia. Đây là task duy nhất sửa `ChartExplore.tsx`, và là nối lại dây chứ không viết lại.

**Files:**
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Thêm state settings và phần tra descriptor**

Sau khai báo `const [cartesian, setCartesian] = useState<CartesianConfig>(...)`, thêm:

```tsx
  const descriptor = getDescriptor(viz);
  // v2 settings for the migrated viz types. The bridge types keep their own useState
  // hooks until PR2/PR3 move them; `settings` is the shell's single opaque store.
  const [settings, setSettings] = useState<VizSettings>({});
```

với các import:

```tsx
import { getDescriptor } from "@/components/chart/viz/registry";
import { toRenderParams } from "@/components/chart/viz/renderParams";
import { StudioPanel } from "@/components/chart/StudioPanel";
import { isParamsV2 } from "@/api/charts";
import type { VizSettings } from "@/api/charts";
```

- [ ] **Step 2: Cho config cartesian đi qua descriptor**

`cartesian` vẫn là object config sống cho chart cartesian (các loại bridge không đụng tới), nhưng đọc/ghi nó giờ qua descriptor. Thay helper `patchCartesian` ở dòng 100 bằng:

```tsx
  const patchCartesian = (p: Partial<CartesianConfig>) => setCartesian((c) => ({ ...c, ...p }));
  // Config the active descriptor works with. Cartesian is the only migrated family in PR1.
  const descConfig = cartesian as never;
  const patchDescConfig = patchCartesian as (p: never) => void;
```

- [ ] **Step 3: Uỷ quyền dựng spec cho cartesian**

Trong `specForRun()`, thay nhánh cartesian (dòng 206-210) bằng:

```tsx
    if (descriptor) {
      return descriptor.buildSpec(descConfig, { filters, rowLimit, temporalUnits });
    }
```

Mọi nhánh khác giữ y nguyên.

- [ ] **Step 4: Uỷ quyền validate cho cartesian**

Ở `validationInput()` / các chỗ gọi `missingFields`, tính nhãn từ descriptor khi có descriptor:

```tsx
  const missingNow = () =>
    descriptor ? descriptor.requiredFields(descConfig) : missingFields(viz, validationInput());
```

Rồi thay hai lời gọi `missingFields(viz, validationInput())` (trong `run()` và trong `querySignature`) bằng `missingNow()`, và lời gọi trong block empty-state bằng `datasetId ? missingNow() : ["a dataset"]`.

- [ ] **Step 5: Uỷ quyền lưu và hydration cho cartesian**

Trong `paramsForSave()`, thêm dòng đầu tiên của hàm:

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

và đổi kiểu trả về thành `AnyChartParams`.

Trong effect hydration, thay nhánh `isCartesianViz(c.viz_type)` bằng:

```tsx
    } else if (getDescriptor(c.viz_type)) {
      setCartesian(getDescriptor(c.viz_type)!.fromParams(p) as CartesianConfig);
      if (isParamsV2(p)) setSettings(p.settings);
```

và đổi ba dòng bên dưới đang đọc `p.columnSettings` / `p.conditionalFormatting` thành đọc qua adapter:

```tsx
    const rp = toRenderParams(c.viz_type, p);
    setColumnSettings(rp.columnSettings ?? {});
    setConditionalFormatting(rp.conditionalFormatting ?? []);
```

(`p.spec` không đổi giữa v1 và v2, nên các dòng `rowLimit` / `temporalUnits` bên dưới vẫn chạy nguyên như cũ.)

- [ ] **Step 6: Vẽ control của descriptor**

Ở tab Data, thay toàn bộ block `{isCartesianViz(viz) && ( … )}` (dòng 673-712) bằng:

```tsx
            {descriptor && (
              <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                unitRow={unitRow} tab="data" />
            )}
```

Ở tab Display, bọc `<ChartSettingsSections … />` hiện có để loại đã chuyển dùng descriptor thay thế:

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

`resultColumns` và `data.rows` đã có sẵn trong component này (dòng 292 và qua `useSpecData`), nên không phải tính thêm gì ở đây — control display của cartesian tự suy ra danh sách series của nó.

- [ ] **Step 7: Cho preview đi qua adapter**

`previewParams()` hiện trả thẳng shape lồng. Cho nó kết thúc bằng adapter để renderer vẫn nhận đầu vào v1:

```tsx
  const previewParams = (): ChartParams => {
    const base = isStale() && ranParams.current ? ranParams.current : paramsForSave();
    return { ...toRenderParams(viz, base), columnSettings: effCS };
  };
```

và đổi `ranParams` thành `useRef<AnyChartParams | null>(null)`.

- [ ] **Step 8: Kiểm tra build và test**

Chạy: `cd frontend && npx tsc --noEmit && npx eslint src/components/chart src/routes/explore/ChartExplore.tsx && npx vitest run`
Mong đợi: `tsc` sạch, eslint sạch (hai warning sẵn có ở `maps/registry.ts` + `options/map.ts` là bình thường), toàn bộ test PASS. Nếu một test trong `src/components/chart/settings/__tests__/ChartSettingsSections.test.tsx` fail, đó là vì nó đang khẳng định về các section cartesian nay đã nằm trong descriptor — chuyển những khẳng định đó sang `viz/__tests__/cartesian.test.tsx` chứ đừng xoá.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): shell drives cartesian through the registry"
```

---

### Task 9: Verify đầy đủ, deploy, live check, PR

**Files:** không có (chỉ kiểm chứng)

- [ ] **Step 1: Cổng FE đầy đủ**

Chạy: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`
Mong đợi: cả bốn xanh; số test tối thiểu là 1268 cộng khoảng 24 test mà plan này thêm vào.

- [ ] **Step 2: Deploy lên kind**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

- [ ] **Step 3: Live check**

Tại `http://egret.localtest.me/explore/demo/charts/new?type=bar` trên dataset Demo Sales:

1. Chọn dataset, đặt X-axis `region` và metric `total_revenue` với Auto bật — chart tự chạy và vẽ bốn cột.
2. Mở tab Display — các control display của cartesian có đủ, và đổi một cái (ví dụ Stacked) khiến chart vẽ lại mà không phát query mới.
3. Save tên "A2 PR1 smoke", tải lại URL đã lưu — nó render ngay khi mở. Xác nhận shape lưu là v2:
   `curl -s http://egret.localtest.me/api/workspaces/demo/charts | jq '.[] | select(.name=="A2 PR1 smoke") | .params | {version, keys: (.settings | keys)}'`
   Mong đợi: `version: 2` và các khoá bắt đầu bằng `cartesian.`.
4. Mở **`A1 studio smoke`** (đã lưu theo v1 trước thay đổi này) — nó phải render y hệt, chứng minh đường fallback v1.
5. Mở một dashboard có card chart, gồm cả card có viz override nếu có — render không đổi.
6. Mở một chart `pie` và một chart `pivot_table` — cả hai vẫn chạy qua bridge.

- [ ] **Step 4: Mở PR (không merge; chờ duyệt)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-studio-a2
gh pr create --base main --title "feat(charts): A2 PR1 — viz registry, studio shell, cartesian descriptor" --body "Implements PR 1 of docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md: params v2 + toRenderParams adapter + registry + StudioPanel/useChartRun + the cartesian family. The other 13 viz types still run on the inline bridge (PR2/PR3). No renderer or dashboard behaviour changes; no SQL migration yet."
```

---

## Ghi chú cho người triển khai

- Bridge là chủ ý, không phải lười: `getDescriptor(viz)` trả về `undefined` chính là cách một viz type nói "vẫn còn inline". Đừng bao giờ thêm descriptor giả để một loại trông như đã chuyển.
- `ChartSettingsSections` còn nằm trong cây tới hết PR3. Đừng xoá ở đây — mười hai loại bridge vẫn vẽ control display bằng nó.
- Nếu `tsc` đẩy bạn tới chỗ phải sửa `ChartRenderer` hay bất cứ thứ gì dưới `echarts/`, hãy dừng: adapter đang thiếu một khoá. Sửa `toRenderParams`.
- Chart v1 đã lưu là đường rủi ro cao nhất trong PR này. Khi phân vân, hãy test với `A1 studio smoke` (một row v1 thật trên cụm kind) thay vì fixture tự dựng.
- Không merge sau khi mở PR — chương trình này chờ duyệt tường minh theo yêu cầu thường trực của người dùng.
