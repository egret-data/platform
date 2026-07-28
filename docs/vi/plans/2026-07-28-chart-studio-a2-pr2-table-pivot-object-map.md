# Chart Studio A2 · PR2 — Kế hoạch triển khai Table, Pivot, Object & Map

> **Cho worker tự động:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai theo từng task. Các step dùng checkbox (`- [ ]`) để theo dõi.

**Bản canonical (tiếng Anh):** `docs/en/plans/2026-07-28-chart-studio-a2-pr2-table-pivot-object-map.md` — code trong hai bản giống hệt nhau; bản này dịch phần diễn giải.

**Mục tiêu:** Move the four most stateful viz types — `table`, `pivot_table`, `object`, `map` — onto descriptors, extend the render adapter to their namespaces, and extract `VizSwitcher` and `PreviewPane` from the shell.

**Kiến trúc:** Exactly the pattern PR1 established (`docs/en/plans/2026-07-28-chart-studio-a2-pr1-registry-cartesian.md`): one module per viz under `components/chart/viz/`, a codec for its `<prefix>.*` settings, `buildSpec`, `requiredFields`, and controls tagged `data` / `display`. The one new piece is `previewConfig()`, which replaces `previewPivotDims` / `previewTableDims` — the stale-freeze rule from #345 now works off the descriptor's config rather than per-viz shell state.

**Tech stack:** React 18, TypeScript, Vite, TanStack Query, shadcn/ui, Vitest + Testing Library.

**Spec:** `docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md` — this is PR 2 of its three.

**Điểm xuất phát:** branch `feat/chart-studio-a2-pr2` off `main` at `0fb7ff58` (PR1 merged). `npm run typecheck` clean, `npm test` green (1305 tests / 233 files).

**Đọc trước — hai cái bẫy PR1 đã vấp, ghi ở cuối plan PR1:**

1. Run tests with `npm test -- <path>`, **never** `npx vitest run <path>` (npx may resolve a cached global vitest with no jsdom).
2. Any helper `useChartRun` reaches during render must be a **function declaration**, not a `const` arrow, or the shell white-screens from the temporal dead zone. `ChartExplore.mount.test.tsx` guards this — keep it green.

**Luật:** no renderer changes (`ChartRenderer`, `echarts/**`, `PivotTable`, `ChartTable`, `MapChart`); no SQL migration (that is PR3); the nine remaining viz types stay on the bridge.

---

### Task 1: Descriptor pivot

Pivot is the biggest v1 clean-up: eight of its fields sit loose at the top level of v1 params and move under `pivot.`.

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
Mong đợi: FAIL — cannot resolve `../pivot`.

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

Both pick raw columns, and in v1 neither stored a config at all — their columns were re-derived from the spec on hydration. They now store them explicitly.

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
Mong đợi: FAIL — cannot resolve `../table`.

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
Mong đợi: FAIL — cannot resolve `../map`.

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

- [ ] **Step 1: Extend the adapter test**

Add to `renderParams.test.ts`:

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

- [ ] **Step 2: Run it to verify it fails**

Chạy: `cd frontend && npm test -- src/components/chart/viz/__tests__/renderParams.test.ts`
Mong đợi: FAIL — `out.rows` and `out.map` are undefined.

- [ ] **Step 3: Extend `toRenderParams`**

Replace the body's tail (after the shared `out` object) with:

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

with imports for `pivotFromParams` and `mapDescriptor`. `table` and `object` need nothing here: v1 carried no config for them, and `ChartTable` / `ObjectDetail` read their columns from the result, not from params.

- [ ] **Step 4: Register the descriptors**

In `registry.ts`, replace the `REGISTRY` initialiser with:

```ts
const REGISTRY: Partial<Record<VizType, VizDescriptor<never>>> = {
  ...Object.fromEntries(CARTESIAN_VIZ.map((k) => [k, cartesianDescriptor(k)])),
  pivot_table: pivotDescriptor(),
  table: tableDescriptor(),
  object: objectDescriptor(),
  map: mapDescriptor(),
} as Partial<Record<VizType, VizDescriptor<never>>>;
```

and update `registry.test.ts`: `hasDescriptor("pie")` stays `false` (pie is PR3), and add `for (const k of ["table", "pivot_table", "object", "map"]) expect(hasDescriptor(k)).toBe(true)`.

- [ ] **Step 5: Run the viz tests**

Chạy: `cd frontend && npm test -- src/components/chart/viz`
Mong đợi: all PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz
git commit -m "feat(charts): register table/pivot/object/map + adapter for their namespaces"
```

---

### Task 5: `previewConfig` thay hai helper freeze theo viz

`previewPivotDims` and `previewTableDims` exist only because pivot and table read shell state instead of a config. With descriptors they collapse into one rule.

**Files:**
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Add the helper**

Next to `previewParams`, add:

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

- [ ] **Step 2: Use it for the pivot and table previews**

Replace `...previewPivotDims(),` in the `PivotTable` input with:

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

Then delete `previewPivotDims`, `previewTableDims` and the `ranTableDims` ref along with its assignment in the `afterRun` callback.

- [ ] **Step 3: Verify**

Chạy: `cd frontend && npx tsc --noEmit && npm test -- src/routes/explore`
Mong đợi: clean and green.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(charts): one preview-freeze rule via previewConfig"
```

---

### Task 6: Xoá bốn nhánh inline

**Files:**
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Route the four configs through the descriptor**

`descConfig` currently hard-codes `cartesian`. Replace it and `patchDescConfig` with a config the shell keeps per active viz:

```tsx
  // One opaque config for whichever viz is active; the descriptor is the only thing that
  // reads inside it. Bridge types still use their own useState hooks below.
  const [descState, setDescState] = useState<unknown>(null);
  const descConfig = (descState ?? descriptor?.defaultConfig) as never;
  const patchDescConfig = (p: object) => setDescState((c) => ({ ...(c as object ?? descriptor?.defaultConfig), ...p })) as never;
```

Hydration sets it with `setDescState(desc.fromParams(p))`; `changeViz` resets it with `setDescState(null)`; `resetControls` does the same. Delete `setCartesian(... as CartesianConfig)` from the hydration branch — `descState` replaces it.

- [ ] **Step 2: Delete the inline branches**

Remove these blocks whole (they are now the descriptors' controls):

- the `{viz === "table" && ( … )}` data block
- the `{viz === "object" && ( … )}` data block
- the `{viz === "map" && ( … )}` data block
- the `{viz === "pivot_table" && ( … )}` data block
- the `{viz === "pivot_table" && ( … )}` **display** block (Totals)

Then delete the state and helpers only they used: `tableMetrics`, `tableCols`, `pRows`, `pCols`, `pVals`, `showRowTotals`, `showColTotals`, `condense`, `collapsed`, `pivotSort`, `mapCfg`, `cartesian`, `toggleTableMetric`, `toggleTableCol`, `unitList`, and the branches for those five viz types in `specForRun`, `paramsForSave`, `validationInput` and the hydration effect.

`collapsed` and `pivotSort` move into the pivot config: the `onToggleCollapse` / `onCycleSort` handlers on `PivotTable` become `patchDescConfig` calls on `collapsedRows` / `sort`.

- [ ] **Step 3: Verify**

Chạy: `cd frontend && npx tsc --noEmit && npx eslint src/routes/explore/ChartExplore.tsx && npm test`
Mong đợi: clean, clean, all green. `ChartExplore.mount.test.tsx` must still pass — it now exercises a v1 pie (still bridged) and a v2 bar.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): drop the table/pivot/object/map inline branches"
```

---

### Task 7: Bóc tách `VizSwitcher` và `PreviewPane`

Mechanical moves, deferred here from PR1.

**Files:**
- Tạo: `frontend/src/components/chart/VizSwitcher.tsx`, `frontend/src/components/chart/PreviewPane.tsx`
- Test: `frontend/src/components/chart/__tests__/VizSwitcher.test.tsx`
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Write the VizSwitcher test**

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

- [ ] **Step 2: Run it to verify it fails**

Chạy: `cd frontend && npm test -- src/components/chart/__tests__/VizSwitcher.test.tsx`
Mong đợi: FAIL — cannot resolve `../VizSwitcher`.

- [ ] **Step 3: Write both components**

`VizSwitcher` owns the chip, the `Change` button and the gallery dialog — move the chip markup near `ChartExplore.tsx:560` and the `<Dialog open={vizPickerOpen}>` block at the end of the file:

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

`PreviewPane` takes the preview block as props and renders the states or the right renderer. Move the JSX from `<div ref={previewRef} className="contents">` through the end of that div, and pass in: `finished`, `isRunning`, `error`, `onRetry`, `viz`, `params`, `columns`, `rows`, `metricTypes`, `emptyState`, plus the pivot/table handlers. Keep the props explicit; do not pass the whole shell.

- [ ] **Step 4: Use them in the shell**

Replace the chip markup with `<VizSwitcher viz={viz} onChange={changeViz} />`, delete the gallery `Dialog` and the `vizPickerOpen` state, and replace the preview block with `<PreviewPane … />`.

- [ ] **Step 5: Verify**

Chạy: `cd frontend && npx tsc --noEmit && npm test`
Mong đợi: clean and green.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/VizSwitcher.tsx frontend/src/components/chart/PreviewPane.tsx frontend/src/components/chart/__tests__/VizSwitcher.test.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(charts): extract VizSwitcher and PreviewPane from the shell"
```

---

### Task 8: Verify đầy đủ, deploy, live check, PR

**Files:** none (verification only)

- [ ] **Step 1: Full FE gate**

Chạy: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`
Mong đợi: all green; test count at least 1305 plus the ~25 this plan adds.

- [ ] **Step 2: Deploy to kind**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

- [ ] **Step 3: Live check**

On the Demo Sales dataset at `http://egret.localtest.me`:

1. **Pivot** — new pivot chart, rows `region`, values `total_revenue`: it runs, totals row shows, the Display tab's three totals toggles work, and collapsing a row group still works.
2. **Pivot freeze** — Auto off, swap rows to `category`: the previous result stays (it must NOT collapse into one blank row — that was #345's bug), then Run switches it.
3. **Table** — metrics + columns produce an aggregated table; unticking the metric gives raw rows.
4. **Object** and **Map** — both build and render.
5. **v1 rows still work** — open `A1 studio smoke` (v1 bar) and the seeded `Revenue by category (pie)` (v1, still bridged).
6. **Dashboard** `A2 PR1 adapter check` renders all three cards unchanged.
7. Confirm the new pivot chart's stored shape:
   `kubectl exec -n egret-system deploy/postgres -- psql -U egret -d egret -t -c "select params->>'version', (select string_agg(k,',' order by k) from jsonb_object_keys(params->'settings') k) from analytics.charts where name='A2 PR2 pivot';"`
   Mong đợi: `2` and keys beginning `pivot.`.

- [ ] **Step 4: Open the PR (do not merge; await approval)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-studio-a2-pr2
gh pr create --base main --title "feat(charts): A2 PR2 — table, pivot, object, map descriptors" --body "PR 2 of docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md. Nine viz types remain on the bridge; PR3 finishes them and adds the 0099 backfill."
```

---

## Ghi chú cho người triển khai

- The nine remaining viz types (pie, scalar, progress, gauge, funnel, waterfall, smartscalar, boxplot, sankey) stay inline. `ChartSettingsSections` and `requiredFields.ts` therefore stay too — PR3 deletes them.
- `collapsedRows` and `sort` are interactive pivot state, not query state: they must not enter the query signature, or clicking a row group would re-run the query. They are in the config but `buildSpec` ignores them, which keeps the signature stable.
- Switching table→object keeps the column selection via `object.columns`' fallback to `table.columns`; that mirrors the shared `tableCols` state this PR deletes.
- If `tsc` pushes you toward editing a renderer, the adapter is missing a key — fix `toRenderParams`.
