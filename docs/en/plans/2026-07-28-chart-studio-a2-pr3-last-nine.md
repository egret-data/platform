# Chart Studio A2 · PR3 — The Last Nine, PreviewPane & the v2 Backfill

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the last nine viz types onto descriptors, delete the bridge, extract `PreviewPane`, and backfill stored params to v2 — finishing sub-project A2.

**Architecture:** The nine are all simple field pickers, so they come from one declarative factory (`fieldDescriptor`) rather than nine hand-written control components: each declares its fields (`dimension` / `metric` / `number` / `boolean`), its spec builder and its required labels. That is much closer to how Metabase and Superset declare controls, without building a general widget framework.

**Tech Stack:** React 18, TypeScript, Vite, Vitest + Testing Library; SQL migration for Postgres.

**Spec:** `docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md` — PR 3 of three.

**Starting point:** branch `feat/chart-studio-a2-pr3` off `main` at `c08051e0`. `npm test` green (1336 tests / 237 files).

**Two corrections to the spec, both settled before this plan:**

1. **`ChartSettingsSections` is NOT deleted.** `CardVizPanel` — the dashboard's per-card viz override editor — still uses it, and overrides are v1-shaped by design (that is why `toRenderParams` exists). It retires with the adapter in sub-project B. What this PR deletes is the *bridge* inside `ChartExplore` and `requiredFields.ts`.
2. **`PreviewPane` lands here**, deferred from PR2, because only now do all viz types have descriptors and the preview's props stop being per-viz.

**Ground rules:** no renderer changes; `npm test -- <path>` (never `npx vitest`); helpers reached by `useChartRun` during render stay function declarations.

---

### Task 1: The field-descriptor factory

**Files:**
- Create: `frontend/src/components/chart/viz/fields.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/fields.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/viz/__tests__/fields.test.tsx
import { describe, it, expect } from "vitest";
import { fieldDescriptor } from "../fields";
import { EMPTY_SPEC } from "@/api/charts";

const d = fieldDescriptor({
  key: "funnel",
  prefix: "funnel.",
  fields: [
    { id: "dimension", label: "Stage (dimension)", kind: "dimension", required: "a dimension", temporal: true },
    { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
  ],
  buildSpec: (cfg, shared, helpers) => helpers.dimsAndMetrics([cfg.dimension], [cfg.metric], shared),
  legacyKey: "funnel",
});

describe("fieldDescriptor", () => {
  it("derives requiredFields from the field list, in order", () => {
    expect(d.requiredFields({})).toEqual(["a dimension", "a metric"]);
    expect(d.requiredFields({ dimension: "region" })).toEqual(["a metric"]);
    expect(d.requiredFields({ dimension: "region", metric: "m" })).toEqual([]);
  });

  it("writes namespaced settings and reads them back", () => {
    const cfg = { dimension: "region", metric: "m" };
    expect(d.toSettings(cfg)).toEqual({ "funnel.dimension": "region", "funnel.metric": "m" });
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  it("reads a v1 chart from its nested key", () => {
    expect(d.fromParams({ spec: EMPTY_SPEC, funnel: { dimension: "region", metric: "m" } }))
      .toEqual({ dimension: "region", metric: "m" });
  });

  it("falls back to the v1 spec when the nested key is missing", () => {
    const cfg = d.fromParams({ spec: { ...EMPTY_SPEC, dimensions: ["region"], metrics: ["m"] } });
    expect(cfg).toEqual({ dimension: "region", metric: "m" });
  });

  it("builds a spec from the declared fields", () => {
    const spec = d.buildSpec({ dimension: "region", metric: "m" }, { filters: [], rowLimit: 500, temporalUnits: {} });
    expect(spec.dimensions).toEqual(["region"]);
    expect(spec.metrics).toEqual(["m"]);
    expect(spec.row_limit).toBe(500);
  });

  it("gives every declared field a data control", () => {
    expect(d.controls.map((c) => c.id)).toEqual(["dimension", "metric"]);
    expect(d.controls.every((c) => c.section === "data")).toBe(true);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/viz/__tests__/fields.test.tsx`
Expected: FAIL — cannot resolve `../fields`.

- [ ] **Step 3: Write the factory**

```tsx
// frontend/src/components/chart/viz/fields.tsx
import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, QuerySpec, VizSettings, VizType } from "@/api/charts";
import type { SharedQueryState, VizControl, VizControlProps, VizDescriptor } from "./types";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

type Cfg = Record<string, unknown>;

export interface FieldSpec {
  id: string;
  label: string;
  kind: "dimension" | "metric" | "number" | "boolean";
  /** Label used in "Add: …" when this field is empty; omit for optional fields. */
  required?: string;
  placeholder?: string;
  /** Show the shared date-grouping row under this field. */
  temporal?: boolean;
  /** Exclude the value of this other field from the options (e.g. sankey source vs target). */
  excludes?: string;
}

export interface FieldDescriptorSpec {
  key: VizType;
  prefix: string;
  fields: FieldSpec[];
  buildSpec: (cfg: Cfg, shared: SharedQueryState, helpers: Helpers) => QuerySpec;
  /** The v1 nested key this viz used, if any (e.g. `funnel`, `boxplot`, `smartscalar`). */
  legacyKey?: string;
  /** Reads a v1 chart that predates the nested key, from its spec. */
  legacyFromSpec?: (spec: QuerySpec) => Cfg;
  display?: VizControl<Cfg>[];
}

interface Helpers {
  dimsAndMetrics: (dims: (unknown)[], metrics: (unknown)[], shared: SharedQueryState) => QuerySpec;
  rawColumns: (cols: (unknown)[], shared: SharedQueryState) => QuerySpec;
}

const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);

const helpers: Helpers = {
  dimsAndMetrics: (dims, metrics, shared) => {
    const d = dims.map(str).filter((x): x is string => !!x);
    return {
      metrics: metrics.map(str).filter((x): x is string => !!x),
      dimensions: d, columns: [], filters: shared.filters, orderby: [],
      row_limit: shared.rowLimit, rollup_groups: [],
      temporal_units: pruneUnits(shared.temporalUnits, d),
    };
  },
  rawColumns: (cols, shared) => ({
    metrics: [], dimensions: [], columns: cols.map(str).filter((x): x is string => !!x),
    filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
  }),
};

function FieldControl({ field, config, onChange, columns, metrics, unitRow }: {
  field: FieldSpec;
} & VizControlProps<Cfg>) {
  const set = (v: unknown) => onChange({ [field.id]: v } as Partial<Cfg>);
  if (field.kind === "boolean") {
    return (
      <div className="flex items-center gap-2">
        <Checkbox id={`f-${field.id}`} checked={!!config[field.id]} onCheckedChange={() => set(!config[field.id])} />
        <Label htmlFor={`f-${field.id}`} className="cursor-pointer text-sm font-normal">{field.label}</Label>
      </div>
    );
  }
  if (field.kind === "number") {
    return (
      <div className="space-y-1">
        <Label htmlFor={`f-${field.id}`} className={LABEL_CLASS}>{field.label}</Label>
        <Input id={`f-${field.id}`} type="number" className="h-8 text-sm" placeholder={field.placeholder}
          value={(config[field.id] as number | undefined) ?? ""}
          onChange={(e) => {
            const n = Number(e.target.value);
            set(e.target.value === "" || Number.isNaN(n) ? undefined : n);
          }} />
      </div>
    );
  }
  const options = field.kind === "metric"
    ? metrics.map((m) => m.metric_name)
    : columns.map((c) => c.column_name).filter((n) => !field.excludes || n !== config[field.excludes]);
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>{field.label}</Label>
        <Select value={(config[field.id] as string) ?? ""} onValueChange={(v) => set(v || undefined)}>
          <SelectTrigger className="h-8 text-sm">
            <SelectValue placeholder={field.placeholder ?? (field.kind === "metric" ? "Pick a metric…" : "Pick a dimension…")} />
          </SelectTrigger>
          <SelectContent>
            {options.map((n) => <SelectItem key={n} value={n}>{n}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {field.temporal && unitRow(config[field.id] as string | undefined)}
    </>
  );
}

/**
 * Builds a descriptor for a viz whose whole configuration is a short list of field
 * pickers. Nine of the nineteen viz types are exactly that, so they are declared rather
 * than hand-written — the same move Metabase and Superset make with their settings and
 * control-panel definitions.
 */
export function fieldDescriptor(spec: FieldDescriptorSpec): VizDescriptor<Cfg> {
  const { key, prefix, fields, legacyKey, legacyFromSpec, display = [] } = spec;
  return {
    key,
    meta: CHART_TYPES.find((t) => t.key === key)!,
    prefix,
    defaultConfig: {},
    fromParams: (p: AnyChartParams): Cfg => {
      if (isParamsV2(p)) {
        const out: Cfg = {};
        for (const f of fields) {
          const v = p.settings[prefix + f.id];
          if (v !== undefined) out[f.id] = v;
        }
        return out;
      }
      const nested = legacyKey ? (p as unknown as Record<string, Cfg | undefined>)[legacyKey] : undefined;
      if (nested) return { ...nested };
      return legacyFromSpec ? legacyFromSpec(p.spec) : {};
    },
    toSettings: (cfg) => {
      const out: VizSettings = {};
      for (const f of fields) if (cfg[f.id] !== undefined) out[prefix + f.id] = cfg[f.id];
      return out;
    },
    requiredFields: (cfg) => fields.filter((f) => f.required && !cfg[f.id]).map((f) => f.required!),
    buildSpec: (cfg, shared) => spec.buildSpec(cfg, shared, helpers),
    controls: [
      ...fields.map((f): VizControl<Cfg> => ({
        id: f.id,
        section: "data",
        render: (props) => <FieldControl field={f} {...props} />,
      })),
      ...display,
    ],
  };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/viz/__tests__/fields.test.tsx`
Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/fields.tsx frontend/src/components/chart/viz/__tests__/fields.test.tsx
git commit -m "feat(charts): declarative field-descriptor factory"
```

---

### Task 2: The nine descriptors

**Files:**
- Create: `frontend/src/components/chart/viz/simple.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/simple.test.tsx`
- Read (the behaviour being preserved): the nine branches of `specForRun`, `ChartExplore.tsx:512-720` for the controls, `requiredFields.ts` for the labels

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/viz/__tests__/simple.test.tsx
import { describe, it, expect } from "vitest";
import { SIMPLE_DESCRIPTORS } from "../simple";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };
const byKey = Object.fromEntries(SIMPLE_DESCRIPTORS.map((d) => [d.key, d]));

describe("the nine simple descriptors", () => {
  it("covers exactly the viz types PR3 migrates", () => {
    expect(SIMPLE_DESCRIPTORS.map((d) => d.key).sort()).toEqual(
      ["boxplot", "funnel", "gauge", "pie", "progress", "sankey", "scalar", "smartscalar", "waterfall"],
    );
  });

  it("keeps pie's required labels", () => {
    expect(byKey.pie.requiredFields({})).toEqual(["a dimension", "a metric"]);
  });

  it("keeps scalar's single-row spec", () => {
    const spec = byKey.scalar.buildSpec({ metric: "m" }, SHARED);
    expect(spec.metrics).toEqual(["m"]);
    expect(spec.row_limit).toBe(1);
  });

  it("keeps smartscalar's ordering by its dimension", () => {
    const spec = byKey.smartscalar.buildSpec({ dimension: "order_date", metric: "m" }, SHARED);
    expect(spec.orderby).toEqual([{ field: "order_date", desc: false }]);
  });

  it("keeps boxplot on raw columns", () => {
    const spec = byKey.boxplot.buildSpec({ category: "region", value: "revenue" }, SHARED);
    expect(spec.columns).toEqual(["region", "revenue"]);
    expect(spec.metrics).toEqual([]);
    expect(spec.dimensions).toEqual([]);
  });

  it("keeps sankey's two dimensions in source→target order", () => {
    const spec = byKey.sankey.buildSpec({ source: "a", target: "b", metric: "m" }, SHARED);
    expect(spec.dimensions).toEqual(["a", "b"]);
  });

  it("round-trips every descriptor's config through settings", () => {
    const sample: Record<string, Record<string, unknown>> = {
      pie: { dimension: "region", metric: "m" },
      scalar: { metric: "m" },
      progress: { metric: "m", goal: 10 },
      gauge: { metric: "m", min: 0, max: 100 },
      funnel: { dimension: "region", metric: "m" },
      waterfall: { xAxis: "region", metric: "m", showTotal: true },
      smartscalar: { dimension: "order_date", metric: "m" },
      boxplot: { category: "region", value: "revenue" },
      sankey: { source: "a", target: "b", metric: "m" },
    };
    for (const d of SIMPLE_DESCRIPTORS) {
      const cfg = sample[d.key];
      expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
    }
  });

  it("reads every descriptor's v1 nested key", () => {
    expect(byKey.waterfall.fromParams({ spec: EMPTY_SPEC, waterfall: { xAxis: "region", metric: "m" } }))
      .toEqual({ xAxis: "region", metric: "m" });
    expect(byKey.boxplot.fromParams({ spec: EMPTY_SPEC, boxplot: { category: "c", value: "v" } }))
      .toEqual({ category: "c", value: "v" });
  });

  it("reads a pre-nested-key v1 chart from its spec", () => {
    expect(byKey.scalar.fromParams({ spec: { ...EMPTY_SPEC, metrics: ["m"] } })).toEqual({ metric: "m" });
    expect(byKey.boxplot.fromParams({ spec: { ...EMPTY_SPEC, columns: ["c", "v"] } }))
      .toEqual({ category: "c", value: "v" });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/viz/__tests__/simple.test.tsx`
Expected: FAIL — cannot resolve `../simple`.

- [ ] **Step 3: Write the nine**

```tsx
// frontend/src/components/chart/viz/simple.tsx
import { PieDisplaySection } from "@/components/chart/settings/PieDisplaySection";
import type { PieConfig, QuerySpec } from "@/api/charts";
import { fieldDescriptor } from "./fields";
import type { VizControl, VizControlProps } from "./types";

type Cfg = Record<string, unknown>;

/** One row per slice value, for the pie's per-slice colour controls. */
function PieDisplayControl({ config, onChange, resultColumns, rows }: VizControlProps<Cfg>) {
  const dimIdx = resultColumns.findIndex((c) => c.name === config.dimension);
  const slices = config.dimension && dimIdx !== -1
    ? Array.from(new Set(rows.map((r) => String((r as unknown[])[dimIdx] ?? "")).filter((v) => v !== "")))
    : [];
  return (
    <PieDisplaySection config={config as PieConfig} sliceNames={slices}
      onChange={(patch) => onChange(patch as Partial<Cfg>)} />
  );
}

const pieDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: PieDisplayControl }];

/** A single-row aggregate — the scalar family's spec. */
const oneRow = (metric: unknown, shared: { filters: []; rowLimit: number }): QuerySpec => ({
  metrics: typeof metric === "string" && metric ? [metric] : [],
  dimensions: [], columns: [], filters: shared.filters, orderby: [], row_limit: 1, rollup_groups: [],
});

export const SIMPLE_DESCRIPTORS = [
  fieldDescriptor({
    key: "pie", prefix: "pie.", legacyKey: "pie", display: pieDisplay,
    fields: [
      { id: "dimension", label: "Dimension", kind: "dimension", required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.dimension], [c.metric], s),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "scalar", prefix: "scalar.", legacyKey: "scalar",
    fields: [{ id: "metric", label: "Metric", kind: "metric", required: "a metric" }],
    buildSpec: (c, s) => oneRow(c.metric, s),
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "progress", prefix: "progress.", legacyKey: "progress",
    fields: [
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
      { id: "goal", label: "Goal", kind: "number", placeholder: "Target value" },
    ],
    buildSpec: (c, s) => oneRow(c.metric, s),
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "gauge", prefix: "gauge.", legacyKey: "gauge",
    fields: [
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
      { id: "min", label: "Min", kind: "number", placeholder: "0" },
      { id: "max", label: "Max", kind: "number", placeholder: "auto" },
    ],
    buildSpec: (c, s) => oneRow(c.metric, s),
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "funnel", prefix: "funnel.", legacyKey: "funnel",
    fields: [
      { id: "dimension", label: "Stage (dimension)", kind: "dimension", required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.dimension], [c.metric], s),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "waterfall", prefix: "waterfall.", legacyKey: "waterfall",
    fields: [
      { id: "xAxis", label: "X-axis (steps)", kind: "dimension", required: "an x-axis", temporal: true },
      { id: "metric", label: "Metric (delta)", kind: "metric", required: "a metric" },
      { id: "showTotal", label: "Show total bar", kind: "boolean" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.xAxis], [c.metric], s),
    legacyFromSpec: (spec) => ({ xAxis: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "smartscalar", prefix: "smartscalar.", legacyKey: "smartscalar",
    fields: [
      { id: "dimension", label: "Time / order dimension", kind: "dimension", required: "a time dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    // Trend needs the rows in dimension order to compare the last two periods.
    buildSpec: (c, s, h) => ({
      ...h.dimsAndMetrics([c.dimension], [c.metric], s),
      orderby: typeof c.dimension === "string" && c.dimension ? [{ field: c.dimension, desc: false }] : [],
    }),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "boxplot", prefix: "boxplot.", legacyKey: "boxplot",
    fields: [
      { id: "category", label: "Category", kind: "dimension", required: "a category", placeholder: "Pick a column…" },
      { id: "value", label: "Value column", kind: "dimension", required: "a value column", placeholder: "Pick a numeric column…" },
    ],
    // Raw rows, no aggregation — quartiles are computed client-side.
    buildSpec: (c, s, h) => h.rawColumns([c.category, c.value], s),
    legacyFromSpec: (spec) => ({ category: spec.columns?.[0], value: spec.columns?.[1] }),
  }),
  fieldDescriptor({
    key: "sankey", prefix: "sankey.", legacyKey: "sankey",
    fields: [
      { id: "source", label: "Source", kind: "dimension", required: "a source" },
      { id: "target", label: "Target", kind: "dimension", required: "a target", excludes: "source" },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.source, c.target], [c.metric], s),
    legacyFromSpec: (spec) => ({ source: spec.dimensions?.[0], target: spec.dimensions?.[1], metric: spec.metrics?.[0] }),
  }),
];
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/viz/__tests__/simple.test.tsx`
Expected: 9 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz/simple.tsx frontend/src/components/chart/viz/__tests__/simple.test.tsx
git commit -m "feat(charts): the last nine viz types as declared descriptors"
```

---

### Task 3: Register them and finish the adapter

**Files:**
- Modify: `frontend/src/components/chart/viz/registry.ts`, `renderParams.ts`, and their tests

- [ ] **Step 1: Extend the tests**

In `registry.test.ts`, replace the bridge assertion with full coverage:

```ts
  it("has a descriptor for every viz type", () => {
    for (const t of CHART_TYPES) expect(hasDescriptor(t.key)).toBe(true);
  });
```

(import `CHART_TYPES` from `@/components/chart/chartTypes`), and in `renderParams.test.ts` add:

```ts
  it("un-flattens a simple viz's nested key", () => {
    const out = toRenderParams("funnel", {
      version: 2, spec: SPEC, settings: { "funnel.dimension": "region", "funnel.metric": "m" },
    });
    expect(out.funnel).toEqual({ dimension: "region", metric: "m" });
  });
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd frontend && npm test -- src/components/chart/viz`
Expected: the two new cases FAIL.

- [ ] **Step 3: Register and extend**

In `registry.ts`, add `...Object.fromEntries(SIMPLE_DESCRIPTORS.map((d) => [d.key, entry(d)]))`.

In `renderParams.ts`, after the pivot and map branches:

```ts
  const simple = SIMPLE_DESCRIPTORS.find((d) => d.key === viz);
  if (simple) {
    // v1 renderers read the nested key (`params.funnel`, `params.boxplot`, …).
    (out as unknown as Record<string, unknown>)[viz === "smartscalar" ? "smartscalar" : viz] = simple.fromParams(params);
  }
```

- [ ] **Step 4: Run the viz tests**

Run: `cd frontend && npm test -- src/components/chart/viz`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/viz
git commit -m "feat(charts): register the last nine + adapter for their namespaces"
```

---

### Task 4: Delete the bridge

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`
- Delete: `frontend/src/components/chart/requiredFields.ts` and its test

- [ ] **Step 1: Remove the nine inline blocks and their state**

Delete the nine `{viz === "…" && ( … )}` data blocks, the `pie`/`scalar`/`progress`/`gauge`/`funnel`/`waterfall`/`smart`/`box`/`sankey` `useState` hooks, `patchPie`, `validationInput`, `applyParamsBack`, and the nine branches in `specForRun` and `paramsForSave` — every one of them now answers through `descriptor`.

`specForRun` and `paramsForSave` collapse to the descriptor path plus the `throw` for an unregistered viz. `missingNow` becomes `descriptor ? descriptor.requiredFields(descConfig) : []`.

The Display tab's `ChartSettingsSections` fallback goes away (every viz has a descriptor), but **the component itself stays** — `CardVizPanel` uses it.

- [ ] **Step 2: Delete `requiredFields.ts`**

```bash
git rm frontend/src/components/chart/requiredFields.ts frontend/src/components/chart/__tests__/requiredFields.test.ts
```

Its labels now live in each descriptor's field list, and its tests are replaced by the per-descriptor `requiredFields` cases.

- [ ] **Step 3: Verify**

Run: `cd frontend && npx tsc --noEmit && npx eslint src/routes/explore/ChartExplore.tsx && npm test`
Expected: clean, clean, green — including `ChartExplore.mount.test.tsx`, whose pie case now goes through a descriptor rather than the bridge.

- [ ] **Step 4: Commit**

```bash
git add -A frontend/src
git commit -m "feat(charts): delete the bridge — every viz now comes from the registry"
```

---

### Task 5: Extract `PreviewPane`

**Files:**
- Create: `frontend/src/components/chart/PreviewPane.tsx`
- Test: `frontend/src/components/chart/__tests__/PreviewPane.test.tsx`
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/__tests__/PreviewPane.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { PreviewPane } from "../PreviewPane";

const base = {
  viz: "bar" as const,
  params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  columns: [], rows: [], metricTypes: {},
  isRunning: false, error: null as string | null, finished: false,
  onRetry: vi.fn(), onCellDrill: vi.fn(),
  emptyTitle: "Set up your Bar", emptyHint: "Add: X-axis, a metric",
  pivot: undefined, table: undefined,
};

describe("PreviewPane", () => {
  it("shows the empty state before a run", () => {
    render(<PreviewPane {...base} />);
    expect(screen.getByText("Set up your Bar")).toBeInTheDocument();
    expect(screen.getByText("Add: X-axis, a metric")).toBeInTheDocument();
  });

  it("shows the loading state while running", () => {
    render(<PreviewPane {...base} isRunning />);
    expect(screen.getByRole("status", { name: /loading chart/i })).toBeInTheDocument();
  });

  it("shows the error with a retry", () => {
    render(<PreviewPane {...base} error="boom" />);
    expect(screen.getByText("boom")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/__tests__/PreviewPane.test.tsx`
Expected: FAIL — cannot resolve `../PreviewPane`.

- [ ] **Step 3: Write it**

Move the JSX from `{data.error && <ChartErrorState …>}` through the end of the `<div ref={previewRef} className="contents">` block into `PreviewPane`, with this interface (explicit props, no shell object):

```tsx
interface Props {
  viz: VizType;
  params: ChartParams;              // already through toRenderParams
  columns: { name: string; type: string }[];
  rows: unknown[][];
  metricTypes: Record<string, string>;
  isRunning: boolean;
  error: string | null;
  finished: boolean;
  onRetry: () => void;
  onCellDrill: (ctx: DrillContext, x: number, y: number) => void;
  emptyTitle: string;
  emptyHint: string;
  emptyIcon?: LucideIcon;
  /** Pivot-only interaction, omitted for every other viz. */
  pivot?: { input: PivotInput; onToggleCollapse: (path: string) => void; onCycleSort: (dim: string) => void };
  /** Table-only state, omitted for every other viz. */
  table?: { state: TableState; onStateChange: (s: TableState) => void; dimensionColumns: string[] };
  containerRef?: React.Ref<HTMLDivElement>;
}
```

The shell passes `pivot` / `table` only for those viz types, so the pane has no per-viz knowledge beyond "render the one I was handed".

- [ ] **Step 4: Use it in the shell**

Replace the preview block with `<PreviewPane … />`, keeping `previewRef` for the export menu.

- [ ] **Step 5: Verify**

Run: `cd frontend && npx tsc --noEmit && npm test`
Expected: clean and green.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/PreviewPane.tsx frontend/src/components/chart/__tests__/PreviewPane.test.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(charts): extract PreviewPane"
```

---

### Task 6: The v2 backfill migration

**Files:**
- Create: `egret/migrations/0099_chart_params_v2.sql`

- [ ] **Step 1: Write the migration**

```sql
-- Chart params v2: the query stays at `spec`, every viz config becomes a flat,
-- namespaced `settings` map (see docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md).
-- Rows already at version 2 are skipped; soft-deleted rows are rewritten too, so restoring
-- one does not resurrect a v1 shape.
UPDATE analytics.charts SET params = jsonb_strip_nulls(
  jsonb_build_object(
    'version', 2,
    'spec', params->'spec',
    'settings',
      COALESCE(
        CASE
          WHEN viz_type IN ('bar','row','line','area','scatter','combo') THEN
            (SELECT jsonb_object_agg('cartesian.' || k, v) FROM jsonb_each(params->'cartesian'))
          WHEN viz_type = 'pivot_table' THEN
            jsonb_strip_nulls(jsonb_build_object(
              'pivot.rows', params->'rows', 'pivot.cols', params->'cols',
              'pivot.metrics', params->'metrics',
              'pivot.showRowTotals', params->'showRowTotals',
              'pivot.showColTotals', params->'showColTotals',
              'pivot.condense', params->'condense',
              'pivot.collapsedRows', params->'collapsedRows',
              'pivot.sort', params->'pivotSort'))
          WHEN viz_type = 'table' THEN
            jsonb_strip_nulls(jsonb_build_object(
              'table.metrics', params->'spec'->'metrics',
              'table.columns', COALESCE(params->'spec'->'dimensions', '[]'::jsonb) || COALESCE(params->'spec'->'columns', '[]'::jsonb)))
          WHEN viz_type = 'object' THEN
            jsonb_build_object('object.columns', COALESCE(params->'spec'->'columns', '[]'::jsonb))
          WHEN viz_type = 'map' THEN
            jsonb_strip_nulls(jsonb_build_object(
              'map.key', COALESCE(params->'map'->'mapKey', '"us_states"'::jsonb),
              'map.region', params->'map'->'region',
              'map.metric', params->'map'->'metric'))
          ELSE
            (SELECT jsonb_object_agg(viz_type || '.' || k, v) FROM jsonb_each(params->viz_type))
        END,
        '{}'::jsonb)
      || jsonb_strip_nulls(jsonb_build_object(
           'column_settings', params->'columnSettings',
           'conditional_formatting', params->'conditionalFormatting'))
  ))
WHERE params->>'version' IS DISTINCT FROM '2';
```

- [ ] **Step 2: Apply it against the kind cluster and diff a chart**

```bash
kubectl exec -n egret-system deploy/postgres -- psql -U egret -d egret -c "\i /dev/stdin" < egret/migrations/0099_chart_params_v2.sql
kubectl exec -n egret-system deploy/postgres -- psql -U egret -d egret -t -c \
  "select name, viz_type, params->>'version', (select string_agg(k,',' order by k) from jsonb_object_keys(params->'settings') k) from analytics.charts order by name;"
```

Expected: every row `2`, keys namespaced for its viz type, and none of the old top-level keys left.

- [ ] **Step 3: Commit**

```bash
git add egret/migrations/0099_chart_params_v2.sql
git commit -m "feat(charts): backfill analytics.charts params to v2"
```

---

### Task 7: Full verify, deploy, live check, PR

- [ ] **Step 1: Full FE gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`

- [ ] **Step 2: Deploy to kind and let the migration run**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

- [ ] **Step 3: Live check**

1. Every one of the nine builds and runs: pie, scalar, progress, gauge, funnel, waterfall, trend, boxplot, sankey — at minimum pie, scalar and sankey end-to-end, the rest to the point where controls render and the chart draws.
2. Charts saved before the migration (`A1 studio smoke`, `Revenue by category (pie)`, `Revenue over time (line)`, `A2 PR1 smoke`, `A2 PR2 pivot`) all open and render after the backfill.
3. The `A2 PR1 adapter check` dashboard renders unchanged, and its card override editor (Edit → a card → viz settings) still shows the display sections.
4. The Display tab still shows Formatting for a bar chart and Columns + Conditional formatting for a table (the #348 guard, live).

- [ ] **Step 4: Open the PR (do not merge; await approval)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-studio-a2-pr3
gh pr create --base main --title "feat(charts): A2 PR3 — the last nine viz types, PreviewPane and the v2 backfill" --body "Finishes sub-project A2."
```

---

## Notes for the implementer

- `ChartSettingsSections` stays: `CardVizPanel` still uses it, and per-card overrides are v1-shaped by design. It retires with `toRenderParams` in sub-project B.
- The migration is the one irreversible step in A2. Run it against kind and diff the rows before committing; `fromParams` still reads v1, so a missed row degrades rather than breaks.
- Boxplot's "category" and "value" are raw columns, not a dimension and a metric — its spec uses `rawColumns`, and its field `kind` is `dimension` only because both pick from the column list.
