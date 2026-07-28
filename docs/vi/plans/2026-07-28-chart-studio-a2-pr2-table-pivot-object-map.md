# Chart Studio A2 · PR2 — Kế hoạch triển khai Table, Pivot, Object & Map

> **Cho worker tự động:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai theo từng task. Các step dùng checkbox (`- [ ]`) để theo dõi.

**Bản canonical (tiếng Anh):** `docs/en/plans/2026-07-28-chart-studio-a2-pr2-table-pivot-object-map.md` — code trong hai bản giống hệt nhau; bản này dịch phần diễn giải.

**Mục tiêu:** Chuyển bốn loại viz nhiều state nhất — `table`, `pivot_table`, `object`, `map` — sang descriptor, mở rộng adapter render cho namespace của chúng, và bóc `VizSwitcher` cùng `PreviewPane` ra khỏi shell.

**Kiến trúc:** Đúng pattern PR1 đã dựng (`docs/vi/plans/2026-07-28-chart-studio-a2-pr1-registry-cartesian.md`): mỗi viz một module dưới `components/chart/viz/`, một codec cho settings `<prefix>.*`, `buildSpec`, `requiredFields`, và control gắn nhãn `data` / `display`. Phần mới duy nhất là `previewConfig()` thay cho `previewPivotDims` / `previewTableDims` — quy tắc đóng băng khi stale của #345 nay chạy trên config của descriptor thay vì state theo từng viz trong shell.

**Tech stack:** React 18, TypeScript, Vite, TanStack Query, shadcn/ui, Vitest + Testing Library.

**Spec:** `docs/vi/specs/2026-07-28-chart-studio-a2-viz-registry-design.md` — đây là PR 2 trong ba PR.

**Điểm xuất phát:** branch `feat/chart-studio-a2-pr2` tách từ `main` tại `0fb7ff58` (PR1 đã merge). `npm run typecheck` sạch, `npm test` xanh (1305 test / 233 file).

**Đọc trước — hai cái bẫy PR1 đã vấp, ghi ở cuối plan PR1:**

1. Chạy test bằng `npm test -- <path>`, **không bao giờ** dùng `npx vitest run <path>` (npx có thể lấy bản vitest cache global không có jsdom).
2. Mọi helper mà `useChartRun` chạm tới lúc render phải là **function declaration**, không phải arrow `const`, nếu không shell trắng màn hình vì temporal dead zone. `ChartExplore.mount.test.tsx` là chốt chặn — giữ nó xanh.

**Luật:** không sửa renderer (`ChartRenderer`, `echarts/**`, `PivotTable`, `ChartTable`, `MapChart`); không có migration SQL (đó là PR3); chín viz còn lại vẫn ở trên bridge.

---

### Task 1: Descriptor pivot

Pivot là chỗ dọn dẹp v1 lớn nhất: tám field của nó nằm rời rạc ở top level của params v1, nay dồn vào `pivot.`.

**Files:**
- Tạo: `frontend/src/components/chart/viz/pivot.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/pivot.test.tsx`
- Đọc: `ChartExplore.tsx:677-687` (data controls), `:920-936` (display controls), the pivot branch of `specForRun`

- [ ] **Step 1: Viết test cho fail**

```tsx
// frontend/src/components/chart/viz/__tests__/pivot.test.tsx
import { describe, it, expect } from "vitest";
import { pivotDescriptor } from "../pivot";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };
const d = pivotDescriptor();

describe("pivotDescriptor", () => {
  it("names the missing fields of an empty config", () => {
    expect(d.requiredFields({ rows: [], cols: [], metrics: [] })).toEqual(["a row dimension", "a value"]);
  });

  it("builds a spec with rows then cols as dimensions and each group as a rollup", () => {
    const spec = d.buildSpec({ rows: ["region"], cols: ["category"], metrics: ["total_revenue"] }, SHARED);
    expect(spec.dimensions).toEqual(["region", "category"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
    expect(spec.rollup_groups).toEqual([["region"], ["category"]]);
  });

  it("omits an empty group from the rollups", () => {
    const spec = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"] }, SHARED);
    expect(spec.rollup_groups).toEqual([["region"]]);
  });

  it("defaults the three totals toggles to on", () => {
    expect(d.defaultConfig.showRowTotals).toBe(true);
    expect(d.defaultConfig.showColTotals).toBe(true);
    expect(d.defaultConfig.condense).toBe(true);
  });

  it("reads a v1 params object from its loose top-level keys", () => {
    const cfg = d.fromParams({
      spec: EMPTY_SPEC, rows: ["region"], cols: [], metrics: ["m"],
      showRowTotals: false, collapsedRows: ["region:North"], pivotSort: { region: "desc" },
    });
    expect(cfg.rows).toEqual(["region"]);
    expect(cfg.showRowTotals).toBe(false);
    expect(cfg.collapsedRows).toEqual(["region:North"]);
    expect(cfg.sort).toEqual({ region: "desc" });
  });

  it("round-trips config → settings → config", () => {
    const cfg = { rows: ["region"], cols: ["category"], metrics: ["m"], showRowTotals: false, showColTotals: true, condense: true, collapsedRows: ["x"], sort: { region: "asc" as const } };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  it("writes only namespaced keys", () => {
    const s = d.toSettings({ rows: ["region"], cols: [], metrics: ["m"] });
    expect(Object.keys(s).every((k) => k.startsWith("pivot."))).toBe(true);
  });
});
```

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/pivot.test.tsx`
Mong đợi: FAIL — không resolve được `../pivot`.

- [ ] **Step 3: Viết descriptor**

```tsx
// frontend/src/components/chart/viz/pivot.tsx
import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FieldZone } from "@/components/chart/config/FieldZone";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, QuerySpec, VizSettings } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

export interface PivotConfig {
  rows: string[];
  cols: string[];
  metrics: string[];
  showRowTotals?: boolean;
  showColTotals?: boolean;
  condense?: boolean;
  /** Serialized collapsed row-group paths — interactive state, persisted like Metabase does. */
  collapsedRows?: string[];
  sort?: Record<string, "asc" | "desc">;
}

const P = "pivot.";
const KEYS = ["rows", "cols", "metrics", "showRowTotals", "showColTotals", "condense", "collapsedRows", "sort"] as const;

export function pivotToSettings(cfg: PivotConfig): VizSettings {
  const out: VizSettings = {};
  for (const k of KEYS) {
    const v = cfg[k];
    if (v !== undefined) out[P + k] = v;
  }
  return out;
}

export function pivotFromParams(params: AnyChartParams): PivotConfig {
  if (isParamsV2(params)) {
    const s = params.settings;
    const out = { rows: [], cols: [], metrics: [] } as PivotConfig;
    for (const k of KEYS) {
      const v = s[P + k];
      if (v !== undefined) (out as unknown as Record<string, unknown>)[k] = v;
    }
    return out;
  }
  // v1 kept these eight loose at the top level of params.
  return {
    rows: params.rows ?? [],
    cols: params.cols ?? [],
    metrics: params.metrics ?? [],
    showRowTotals: params.showRowTotals ?? true,
    showColTotals: params.showColTotals ?? true,
    condense: params.condense ?? true,
    collapsedRows: params.collapsedRows ?? [],
    sort: params.pivotSort ?? {},
  };
}

function RowsControl({ config, onChange, columns }: VizControlProps<PivotConfig>) {
  return (
    <FieldZone label="Rows" selected={config.rows} onChange={(v) => onChange({ rows: v })}
      options={columns.map((c) => c.column_name).filter((n) => !config.cols.includes(n))} />
  );
}

function ColsControl({ config, onChange, columns, unitRow }: VizControlProps<PivotConfig>) {
  return (
    <>
      <FieldZone label="Columns" selected={config.cols} onChange={(v) => onChange({ cols: v })}
        options={columns.map((c) => c.column_name).filter((n) => !config.rows.includes(n))} />
      {[...config.rows, ...config.cols].map((c) => unitRow(c))}
    </>
  );
}

function ValuesControl({ config, onChange, metrics }: VizControlProps<PivotConfig>) {
  return (
    <FieldZone label="Values" selected={config.metrics} onChange={(v) => onChange({ metrics: v })}
      options={metrics.map((m) => m.metric_name)} />
  );
}

function TotalsControl({ config, onChange }: VizControlProps<PivotConfig>) {
  const row = (id: string, label: string, checked: boolean, key: keyof PivotConfig) => (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} checked={checked} onCheckedChange={(v) => onChange({ [key]: v } as Partial<PivotConfig>)} />
    </div>
  );
  return (
    <div className="space-y-1">
      <SectionLabel>Totals</SectionLabel>
      <div className="space-y-2.5 pt-1.5">
        {row("show-row-totals", "Row totals", config.showRowTotals ?? true, "showRowTotals")}
        {row("show-col-totals", "Column totals", config.showColTotals ?? true, "showColTotals")}
        {row("condense-totals", "Condense duplicate totals", config.condense ?? true, "condense")}
      </div>
    </div>
  );
}

export function pivotDescriptor(): VizDescriptor<PivotConfig> {
  return {
    key: "pivot_table",
    meta: CHART_TYPES.find((t) => t.key === "pivot_table")!,
    prefix: P,
    defaultConfig: { rows: [], cols: [], metrics: [], showRowTotals: true, showColTotals: true, condense: true },
    fromParams: pivotFromParams,
    toSettings: pivotToSettings,
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.rows.length) out.push("a row dimension");
      if (!cfg.metrics.length) out.push("a value");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const dims = [...cfg.rows, ...cfg.cols];
      return {
        metrics: cfg.metrics, dimensions: dims, columns: [], filters: shared.filters, orderby: [],
        row_limit: shared.rowLimit,
        rollup_groups: [cfg.rows, cfg.cols].filter((g) => g.length > 0),
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls: [
      { id: "rows", section: "data", render: RowsControl },
      { id: "cols", section: "data", render: ColsControl },
      { id: "values", section: "data", render: ValuesControl },
      { id: "totals", section: "display", render: TotalsControl },
    ],
  };
}
```

- [ ] **Step 4: Chạy test để xác nhận nó pass**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/pivot.test.tsx`
Mong đợi: 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/pivot.tsx frontend/src/components/chart/viz/__tests__/pivot.test.tsx
git commit -m "feat(charts): pivot viz descriptor"
```

---

### Task 2: Descriptor table và object

Cả hai đều chọn cột thô, và ở v1 không loại nào lưu config — cột của chúng được suy lại từ spec lúc hydrate. Nay lưu tường minh.

**Files:**
- Tạo: `frontend/src/components/chart/viz/table.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/table.test.tsx`
- Đọc: `ChartExplore.tsx:599-642` (data controls), the table/object branches of `specForRun`

- [ ] **Step 1: Viết test cho fail**

```tsx
// frontend/src/components/chart/viz/__tests__/table.test.tsx
import { describe, it, expect } from "vitest";
import { tableDescriptor, objectDescriptor } from "../table";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };

describe("tableDescriptor", () => {
  const d = tableDescriptor();

  it("needs nothing — a bare table lists raw rows", () => {
    expect(d.requiredFields({ columns: [], metrics: [] })).toEqual([]);
  });

  it("groups by the chosen columns when metrics are picked", () => {
    const spec = d.buildSpec({ columns: ["region"], metrics: ["total_revenue"] }, SHARED);
    expect(spec.dimensions).toEqual(["region"]);
    expect(spec.columns).toEqual([]);
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("selects raw columns when no metric is picked", () => {
    const spec = d.buildSpec({ columns: ["region", "category"], metrics: [] }, SHARED);
    expect(spec.dimensions).toEqual([]);
    expect(spec.columns).toEqual(["region", "category"]);
  });

  it("reads a v1 table chart back out of its spec", () => {
    const cfg = d.fromParams({ spec: { ...EMPTY_SPEC, metrics: ["m"], dimensions: ["region"], columns: [] } });
    expect(cfg).toEqual({ columns: ["region"], metrics: ["m"] });
  });

  it("round-trips config → settings → config", () => {
    const cfg = { columns: ["region"], metrics: ["m"] };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });
});

describe("objectDescriptor", () => {
  const d = objectDescriptor();

  it("needs at least one column", () => {
    expect(d.requiredFields({ columns: [] })).toEqual(["at least one column"]);
    expect(d.requiredFields({ columns: ["region"] })).toEqual([]);
  });

  it("selects its columns raw", () => {
    const spec = d.buildSpec({ columns: ["region"] }, SHARED);
    expect(spec.columns).toEqual(["region"]);
    expect(spec.metrics).toEqual([]);
  });

  it("reads a v1 object chart from its spec columns", () => {
    expect(d.fromParams({ spec: { ...EMPTY_SPEC, columns: ["a", "b"] } })).toEqual({ columns: ["a", "b"] });
  });

  it("falls back to the table columns so switching table→object keeps the selection", () => {
    const cfg = d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: { "table.columns": ["region"] } });
    expect(cfg).toEqual({ columns: ["region"] });
  });
});
```

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/table.test.tsx`
Mong đợi: FAIL — không resolve được `../table`.

- [ ] **Step 3: Viết các descriptor**

```tsx
// frontend/src/components/chart/viz/table.tsx
import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, QuerySpec, VizSettings } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

export interface TableConfig { columns: string[]; metrics: string[] }
export interface ObjectConfig { columns: string[] }

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

/** A checkbox list — the shape both of these viz types configure with. */
function CheckList({ idPrefix, empty, options, selected, onToggle }: {
  idPrefix: string; empty: string; options: string[]; selected: string[]; onToggle: (name: string) => void;
}) {
  return (
    <div className="space-y-1 pt-1">
      {options.length === 0 && <p className="text-xs italic text-muted-foreground">{empty}</p>}
      {options.map((name) => (
        <div key={name} className="flex items-center gap-2">
          <Checkbox id={`${idPrefix}-${name}`} checked={selected.includes(name)} onCheckedChange={() => onToggle(name)} />
          <Label htmlFor={`${idPrefix}-${name}`} className="cursor-pointer text-sm font-normal">{name}</Label>
        </div>
      ))}
    </div>
  );
}

const toggle = (list: string[], name: string) =>
  list.includes(name) ? list.filter((x) => x !== name) : [...list, name];

function TableMetricsControl({ config, onChange, metrics }: VizControlProps<TableConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Metrics</Label>
      <p className="text-xs text-muted-foreground">Pick metrics for an aggregated table, or just columns for raw rows.</p>
      <CheckList idPrefix="metric" empty="No metrics on this dataset."
        options={metrics.map((m) => m.metric_name)} selected={config.metrics}
        onToggle={(n) => onChange({ metrics: toggle(config.metrics, n) })} />
    </div>
  );
}

function TableColumnsControl({ config, onChange, columns, unitRow }: VizControlProps<TableConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Columns</Label>
        <CheckList idPrefix="col" empty="Select a dataset to see columns."
          options={columns.map((c) => c.column_name)} selected={config.columns}
          onToggle={(n) => onChange({ columns: toggle(config.columns, n) })} />
      </div>
      {config.metrics.length > 0 && config.columns.map((c) => unitRow(c))}
    </>
  );
}

function ObjectFieldsControl({ config, onChange, columns }: VizControlProps<ObjectConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Fields</Label>
      <CheckList idPrefix="obj" empty="Select a dataset to see columns."
        options={columns.map((c) => c.column_name)} selected={config.columns}
        onToggle={(n) => onChange({ columns: toggle(config.columns, n) })} />
    </div>
  );
}

export function tableDescriptor(): VizDescriptor<TableConfig> {
  return {
    key: "table",
    meta: CHART_TYPES.find((t) => t.key === "table")!,
    prefix: "table.",
    defaultConfig: { columns: [], metrics: [] },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        return {
          columns: (p.settings["table.columns"] as string[]) ?? [],
          metrics: (p.settings["table.metrics"] as string[]) ?? [],
        };
      }
      // v1 stored no table config: the columns were re-derived from the spec.
      return {
        metrics: p.spec?.metrics ?? [],
        columns: [...(p.spec?.dimensions ?? []), ...(p.spec?.columns ?? [])],
      };
    },
    toSettings: (cfg) => ({ "table.columns": cfg.columns, "table.metrics": cfg.metrics }),
    requiredFields: () => [],
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const agg = cfg.metrics.length > 0;
      const dims = agg ? cfg.columns : [];
      return {
        metrics: cfg.metrics, dimensions: dims, columns: agg ? [] : cfg.columns,
        filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls: [
      { id: "metrics", section: "data", render: TableMetricsControl },
      { id: "columns", section: "data", render: TableColumnsControl },
    ],
  };
}

export function objectDescriptor(): VizDescriptor<ObjectConfig> {
  return {
    key: "object",
    meta: CHART_TYPES.find((t) => t.key === "object")!,
    prefix: "object.",
    defaultConfig: { columns: [] },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        // Falls back to `table.columns` so switching table→object keeps the selection,
        // which is what the shared `tableCols` state did before A2.
        const own = p.settings["object.columns"] as string[] | undefined;
        return { columns: own ?? (p.settings["table.columns"] as string[]) ?? [] };
      }
      return { columns: p.spec?.columns ?? [] };
    },
    toSettings: (cfg) => ({ "object.columns": cfg.columns }),
    requiredFields: (cfg) => (cfg.columns.length ? [] : ["at least one column"]),
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => ({
      metrics: [], dimensions: [], columns: cfg.columns,
      filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
    }),
    controls: [{ id: "fields", section: "data", render: ObjectFieldsControl }],
  };
}
```

- [ ] **Step 4: Chạy test để xác nhận nó pass**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/table.test.tsx`
Mong đợi: 9 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/table.tsx frontend/src/components/chart/viz/__tests__/table.test.tsx
git commit -m "feat(charts): table + object viz descriptors"
```

---

### Task 3: Descriptor map

**Files:**
- Tạo: `frontend/src/components/chart/viz/map.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/map.test.tsx`
- Đọc: `ChartExplore.tsx:645-676` (data controls), the map branch of `specForRun`

- [ ] **Step 1: Viết test cho fail**

```tsx
// frontend/src/components/chart/viz/__tests__/map.test.tsx
import { describe, it, expect } from "vitest";
import { mapDescriptor } from "../map";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: {} };
const d = mapDescriptor();

describe("mapDescriptor", () => {
  it("names both missing fields", () => {
    expect(d.requiredFields({ mapKey: "us_states" })).toEqual(["a region", "a metric"]);
  });

  it("defaults to the us_states map", () => {
    expect(d.defaultConfig.mapKey).toBe("us_states");
  });

  it("builds a spec grouping by the region column", () => {
    const spec = d.buildSpec({ mapKey: "world", region: "country", metric: "total_revenue" }, SHARED);
    expect(spec.dimensions).toEqual(["country"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("reads a v1 map chart from its nested key", () => {
    expect(d.fromParams({ spec: EMPTY_SPEC, map: { mapKey: "world", region: "country", metric: "m" } }))
      .toEqual({ mapKey: "world", region: "country", metric: "m" });
  });

  it("round-trips config → settings → config", () => {
    const cfg = { mapKey: "vietnam", region: "province", metric: "m" };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });
});
```

- [ ] **Step 2: Chạy test để xác nhận nó fail**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/map.test.tsx`
Mong đợi: FAIL — không resolve được `../map`.

- [ ] **Step 3: Viết descriptor**

```tsx
// frontend/src/components/chart/viz/map.tsx
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { MAPS } from "@/components/chart/echarts/maps/registry";
import { Label } from "@/components/ui/label";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { isParamsV2 } from "@/api/charts";
import type { MapConfig, QuerySpec } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

function MapControls({ config, onChange, columns, metrics }: VizControlProps<MapConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Map</Label>
        <Select value={config.mapKey} onValueChange={(v) => onChange({ mapKey: v })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
          <SelectContent>
            {Object.values(MAPS).map((m) => <SelectItem key={m.key} value={m.key}>{m.label}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Region</Label>
        <Select value={config.region ?? ""} onValueChange={(v) => onChange({ region: v || undefined })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a region column…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Metric</Label>
        <Select value={config.metric ?? ""} onValueChange={(v) => onChange({ metric: v || undefined })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
          <SelectContent>
            {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
    </>
  );
}

export function mapDescriptor(): VizDescriptor<MapConfig> {
  return {
    key: "map",
    meta: CHART_TYPES.find((t) => t.key === "map")!,
    prefix: "map.",
    defaultConfig: { mapKey: "us_states" },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        return {
          mapKey: (p.settings["map.key"] as string) ?? "us_states",
          region: p.settings["map.region"] as string | undefined,
          metric: p.settings["map.metric"] as string | undefined,
        };
      }
      return p.map ?? { mapKey: "us_states" };
    },
    toSettings: (cfg) => {
      const out: Record<string, unknown> = { "map.key": cfg.mapKey };
      if (cfg.region !== undefined) out["map.region"] = cfg.region;
      if (cfg.metric !== undefined) out["map.metric"] = cfg.metric;
      return out;
    },
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.region) out.push("a region");
      if (!cfg.metric) out.push("a metric");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => ({
      metrics: cfg.metric ? [cfg.metric] : [],
      dimensions: cfg.region ? [cfg.region] : [],
      columns: [], filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
    }),
    controls: [{ id: "map", section: "data", render: MapControls }],
  };
}
```

- [ ] **Step 4: Chạy test để xác nhận nó pass**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/map.test.tsx`
Mong đợi: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/map.tsx frontend/src/components/chart/viz/__tests__/map.test.tsx
git commit -m "feat(charts): map viz descriptor"
```

---

### Task 4: Đăng ký bốn descriptor và mở rộng adapter

**Files:**
- Sửa: `frontend/src/components/chart/viz/registry.ts`
- Sửa: `frontend/src/components/chart/viz/renderParams.ts`
- Sửa: `frontend/src/components/chart/viz/__tests__/renderParams.test.ts`

- [ ] **Step 1: Mở rộng test của adapter**

Thêm vào `renderParams.test.ts`:

```ts
  it("un-flattens the pivot keys back to v1's loose top-level fields", () => {
    const out = toRenderParams("pivot_table", {
      version: 2, spec: SPEC,
      settings: { "pivot.rows": ["region"], "pivot.cols": [], "pivot.metrics": ["m"], "pivot.showRowTotals": false, "pivot.collapsedRows": ["x"], "pivot.sort": { region: "asc" } },
    });
    expect(out.rows).toEqual(["region"]);
    expect(out.metrics).toEqual(["m"]);
    expect(out.showRowTotals).toBe(false);
    expect(out.collapsedRows).toEqual(["x"]);
    expect(out.pivotSort).toEqual({ region: "asc" });
  });

  it("un-flattens the map keys", () => {
    const out = toRenderParams("map", {
      version: 2, spec: SPEC, settings: { "map.key": "world", "map.region": "country", "map.metric": "m" },
    });
    expect(out.map).toEqual({ mapKey: "world", region: "country", metric: "m" });
  });
```

- [ ] **Step 2: Chạy để xác nhận nó fail**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/renderParams.test.ts`
Mong đợi: FAIL — `out.rows` và `out.map` đều undefined.

- [ ] **Step 3: Mở rộng `toRenderParams`**

Thay phần đuôi của hàm (sau object `out` dùng chung) bằng:

```ts
  if (isCartesianViz(viz)) out.cartesian = cartesianFromSettings(s);
  if (viz === "pivot_table") {
    const p = pivotFromParams(params);
    out.rows = p.rows; out.cols = p.cols; out.metrics = p.metrics;
    out.showRowTotals = p.showRowTotals; out.showColTotals = p.showColTotals; out.condense = p.condense;
    out.collapsedRows = p.collapsedRows; out.pivotSort = p.sort;
  }
  if (viz === "map") out.map = mapDescriptor().fromParams(params);
  return out;
```

kèm import `pivotFromParams` và `mapDescriptor`. `table` và `object` không cần gì ở đây: v1 không mang config cho chúng, và `ChartTable` / `ObjectDetail` lấy cột từ kết quả chứ không từ params.

- [ ] **Step 4: Đăng ký các descriptor**

Trong `registry.ts`, thay phần khởi tạo `REGISTRY` bằng:

```ts
const REGISTRY: Partial<Record<VizType, VizDescriptor<never>>> = {
  ...Object.fromEntries(CARTESIAN_VIZ.map((k) => [k, cartesianDescriptor(k)])),
  pivot_table: pivotDescriptor(),
  table: tableDescriptor(),
  object: objectDescriptor(),
  map: mapDescriptor(),
} as Partial<Record<VizType, VizDescriptor<never>>>;
```

và cập nhật `registry.test.ts`: `hasDescriptor("pie")` stays `false` (pie is PR3), and add `for (const k of ["table", "pivot_table", "object", "map"]) expect(hasDescriptor(k)).toBe(true)`.

- [ ] **Step 5: Chạy test viz**

Chạy: `cd frontend && npm test -- src/components/chart/viz`
Mong đợi: all PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz
git commit -m "feat(charts): register table/pivot/object/map + adapter for their namespaces"
```

---

### Task 5: `previewConfig` thay hai helper freeze theo viz

`previewPivotDims` và `previewTableDims` tồn tại chỉ vì pivot và table đọc state của shell thay vì một config. Có descriptor rồi thì chúng gộp làm một quy tắc.

**Files:**
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Thêm helper**

Cạnh `previewParams`, thêm:

```tsx
  // The config the preview renders with: frozen at what last ran while the query is stale,
  // exactly like previewParams. Field names inside it address the columns of the result on
  // screen, so pairing fresh names with old columns is what #345 fixed for charts — this is
  // the same rule for the viz types whose preview takes a config instead of ChartParams.
  function previewConfig<Cfg>(desc: VizDescriptor<Cfg>): Cfg {
    const base = isStale && ranParams.current ? ranParams.current : paramsForSave();
    return desc.fromParams(base);
  }
```

- [ ] **Step 2: Dùng nó cho preview pivot và table**

Thay `...previewPivotDims(),` trong input của `PivotTable` bằng:

```tsx
              ...(() => {
                const c = previewConfig(pivotDescriptor());
                return { rowDims: c.rows, colDims: c.cols, metrics: c.metrics };
              })(),
```

and `dimensionColumns={previewTableDims()}` with:

```tsx
            dimensionColumns={(() => {
              const c = previewConfig(tableDescriptor());
              return c.metrics.length > 0 ? c.columns : [];
            })()}
```

Rồi xoá `previewPivotDims`, `previewTableDims` và ref `ranTableDims` cùng dòng gán nó trong callback `afterRun`.

- [ ] **Step 3: Kiểm chứng**

Chạy: `cd frontend && npx tsc --noEmit && npm test -- src/routes/explore`
Mong đợi: sạch và xanh.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(charts): one preview-freeze rule via previewConfig"
```

---

### Task 6: Xoá bốn nhánh inline

**Files:**
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Cho bốn config đi qua descriptor**

`descConfig` hiện hard-code `cartesian`. Thay nó và `patchDescConfig` bằng một config shell giữ theo viz đang hoạt động:

```tsx
  // One opaque config for whichever viz is active; the descriptor is the only thing that
  // reads inside it. Bridge types still use their own useState hooks below.
  const [descState, setDescState] = useState<unknown>(null);
  const descConfig = (descState ?? descriptor?.defaultConfig) as never;
  const patchDescConfig = (p: object) => setDescState((c) => ({ ...(c as object ?? descriptor?.defaultConfig), ...p })) as never;
```

Hydration set nó bằng `setDescState(desc.fromParams(p))`; `changeViz` reset bằng `setDescState(null)`; `resetControls` cũng vậy. Xoá `setCartesian(... as CartesianConfig)` khỏi nhánh hydration — `descState` thay thế nó.

- [ ] **Step 2: Xoá các nhánh inline**

Xoá trọn các block sau (nay là control của descriptor):

- the `{viz === "table" && ( … )}` data block
- the `{viz === "object" && ( … )}` data block
- the `{viz === "map" && ( … )}` data block
- the `{viz === "pivot_table" && ( … )}` data block
- the `{viz === "pivot_table" && ( … )}` **display** block (Totals)

Rồi xoá state và helper chỉ chúng dùng: `tableMetrics`, `tableCols`, `pRows`, `pCols`, `pVals`, `showRowTotals`, `showColTotals`, `condense`, `collapsed`, `pivotSort`, `mapCfg`, `cartesian`, `toggleTableMetric`, `toggleTableCol`, `unitList`, cùng các nhánh của năm loại viz đó trong `specForRun`, `paramsForSave`, `validationInput` và effect hydration.

`collapsed` và `pivotSort` chuyển vào config của pivot: handler `onToggleCollapse` / `onCycleSort` của `PivotTable` trở thành lời gọi `patchDescConfig` lên `collapsedRows` / `sort`.

- [ ] **Step 3: Kiểm chứng**

Chạy: `cd frontend && npx tsc --noEmit && npx eslint src/routes/explore/ChartExplore.tsx && npm test`
Mong đợi: sạch, sạch, tất cả xanh. `ChartExplore.mount.test.tsx` vẫn phải pass — nó đang chạy một pie v1 (còn trên bridge) và một bar v2.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): drop the table/pivot/object/map inline branches"
```

---

### Task 7: Bóc tách `VizSwitcher` và `PreviewPane`

Bóc tách cơ học, hoãn từ PR1 sang.

**Files:**
- Tạo: `frontend/src/components/chart/VizSwitcher.tsx`, `frontend/src/components/chart/PreviewPane.tsx`
- Test: `frontend/src/components/chart/__tests__/VizSwitcher.test.tsx`
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Viết test cho VizSwitcher**

```tsx
// frontend/src/components/chart/__tests__/VizSwitcher.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { VizSwitcher } from "../VizSwitcher";

describe("VizSwitcher", () => {
  it("shows the active viz name", () => {
    render(<VizSwitcher viz="bar" onChange={vi.fn()} />);
    expect(screen.getByText("Bar")).toBeInTheDocument();
  });

  it("opens the gallery and reports the chosen type", async () => {
    const onChange = vi.fn();
    render(<VizSwitcher viz="bar" onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: /change/i }));
    await userEvent.click(await screen.findByText("Pie"));
    expect(onChange).toHaveBeenCalledWith("pie");
  });
});
```

- [ ] **Step 2: Chạy để xác nhận nó fail**

Chạy: `cd frontend && npm test -- src/components/chart/__tests__/VizSwitcher.test.tsx`
Mong đợi: FAIL — không resolve được `../VizSwitcher`.

- [ ] **Step 3: Viết cả hai component**

`VizSwitcher` sở hữu chip, nút `Change` và dialog gallery — chuyển markup chip quanh `ChartExplore.tsx:560` và block `<Dialog open={vizPickerOpen}>` ở cuối file:

```tsx
// frontend/src/components/chart/VizSwitcher.tsx
import { useState } from "react";
import { CHART_TYPES } from "./chartTypes";
import { ChartTypeGallery } from "./ChartTypeGallery";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { VizType } from "@/api/charts";

export function VizSwitcher({ viz, onChange }: { viz: VizType; onChange: (v: VizType) => void }) {
  const [open, setOpen] = useState(false);
  const meta = CHART_TYPES.find((t) => t.key === viz);
  const Icon = meta?.icon;
  return (
    <div className="flex items-center justify-between">
      <div className="flex items-center gap-2">
        {Icon && <Icon className="h-4 w-4 text-primary" aria-hidden />}
        <span className="text-sm font-medium">{meta?.name}</span>
      </div>
      <Button variant="ghost" size="sm" className="h-7 px-2 text-xs" onClick={() => setOpen(true)}>Change</Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader><DialogTitle>Choose a visualization</DialogTitle></DialogHeader>
          <ChartTypeGallery onSelect={(k) => { onChange(k); setOpen(false); }} />
        </DialogContent>
      </Dialog>
    </div>
  );
}
```

`PreviewPane` nhận block preview qua props và vẽ ra trạng thái hoặc renderer phù hợp. Chuyển JSX từ `<div ref={previewRef} className="contents">` tới hết div đó, và truyền vào: `finished`, `isRunning`, `error`, `onRetry`, `viz`, `params`, `columns`, `rows`, `metricTypes`, `emptyState`, cộng các handler của pivot/table. Giữ props tường minh; đừng truyền cả shell.

- [ ] **Step 4: Dùng chúng trong shell**

Thay markup của chip bằng `<VizSwitcher viz={viz} onChange={changeViz} />`, xoá `Dialog` gallery và state `vizPickerOpen`, và thay block preview bằng `<PreviewPane … />`.

- [ ] **Step 5: Kiểm chứng**

Chạy: `cd frontend && npx tsc --noEmit && npm test`
Mong đợi: sạch và xanh.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/VizSwitcher.tsx frontend/src/components/chart/PreviewPane.tsx frontend/src/components/chart/__tests__/VizSwitcher.test.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(charts): extract VizSwitcher and PreviewPane from the shell"
```

---

### Task 8: Verify đầy đủ, deploy, live check, PR

**Files:** không có (chỉ kiểm chứng)

- [ ] **Step 1: Cổng FE đầy đủ**

Chạy: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`
Mong đợi: tất cả xanh; số test tối thiểu 1305 cộng khoảng 25 test plan này thêm vào.

- [ ] **Step 2: Deploy lên kind**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

- [ ] **Step 3: Live check**

Trên dataset Demo Sales tại `http://egret.localtest.me`:

1. **Pivot** — new pivot chart, rows `region`, values `total_revenue`: it runs, totals row shows, the Display tab's three totals toggles work, and collapsing a row group still works.
2. **Pivot freeze** — Auto off, swap rows to `category`: the previous result stays (it must NOT collapse into one blank row — that was #345's bug), then Run switches it.
3. **Table** — metrics + columns produce an aggregated table; unticking the metric gives raw rows.
4. **Object** and **Map** — both build and render.
5. **v1 rows still work** — open `A1 studio smoke` (v1 bar) and the seeded `Revenue by category (pie)` (v1, still bridged).
6. **Dashboard** `A2 PR1 adapter check` renders all three cards unchanged.
7. Confirm the new pivot chart's stored shape:
   `kubectl exec -n egret-system deploy/postgres -- psql -U egret -d egret -t -c "select params->>'version', (select string_agg(k,',' order by k) from jsonb_object_keys(params->'settings') k) from analytics.charts where name='A2 PR2 pivot';"`
   Mong đợi: `2` and keys beginning `pivot.`.

- [ ] **Step 4: Mở PR (không merge; chờ duyệt)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-studio-a2-pr2
gh pr create --base main --title "feat(charts): A2 PR2 — table, pivot, object, map descriptors" --body "PR 2 of docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md. Nine viz types remain on the bridge; PR3 finishes them and adds the 0099 backfill."
```

---

## Ghi chú cho người triển khai

- Chín viz còn lại (pie, scalar, progress, gauge, funnel, waterfall, smartscalar, boxplot, sankey) vẫn inline. Nên `ChartSettingsSections` và `requiredFields.ts` cũng còn — PR3 mới xoá.
- `collapsedRows` và `sort` là state tương tác của pivot, không phải state truy vấn: chúng không được vào query signature, nếu không bấm gập một nhóm dòng sẽ chạy lại query. Chúng nằm trong config nhưng `buildSpec` bỏ qua, nhờ vậy signature ổn định.
- Đổi table→object vẫn giữ cột đã chọn nhờ `object.columns` fallback sang `table.columns`; đúng như state `tableCols` dùng chung mà PR này xoá đi.
- Nếu `tsc` đẩy bạn tới chỗ sửa renderer thì adapter đang thiếu khoá — sửa `toRenderParams`.
