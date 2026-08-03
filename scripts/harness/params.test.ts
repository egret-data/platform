/**
 * The contract this file defends: `harnessParams` must produce byte-identical output to the
 * chart studio's own `paramsForSave()` (frontend/src/routes/explore/ChartExplore.tsx).
 *
 * Every fixture here is a chart the hand-written harness actually got WRONG:
 *   - pie    — was written with `pie.dimension` (singular). The descriptor emits `pie.dimensions`.
 *   - pivot  — was written with `spec.rollup_groups: []`. The descriptor emits the real groups.
 *   - table  — was written with `settings: {}`, so it opened on "Set up your Table".
 *   - line   — was written with `temporal_units: {}`, making a whole PR's work inert.
 *
 * The expectations are therefore never hand-typed literals of what the settings "should" be;
 * they are the descriptor calls themselves. A hand-typed literal is how all four bugs got in.
 */
// `describe`/`it`/`expect` come from vitest's globals (vitest.config.ts `globals: true`,
// tsconfig `types: ["vitest/globals"]`) rather than an explicit import: this file sits outside
// `frontend/`, so a bare `from "vitest"` walks up from the repo root and never finds
// `frontend/node_modules`. `@/…` still resolves, through tsconfig `paths`.
import { getDescriptor } from "@/components/chart/viz/registry";
import { mergeVizSettings } from "@/components/chart/viz/renderConfig";
import type { VizType } from "@/api/charts";
import { harnessParams, type HarnessChart } from "./params";

/** The dataset shape the generator needs: which columns are dates. */
const COLUMNS = [
  { column_name: "category", is_temporal: false },
  { column_name: "state", is_temporal: false },
  { column_name: "created_at", is_temporal: true },
];

/** What ChartExplore's own state starts at for a brand-new chart. */
const SHARED = { filters: [], rowLimit: 1000, temporalUnits: {} };

/**
 * Exactly what `paramsForSave()` does, spelled out here so the test compares against the
 * studio's four calls rather than against `harnessParams`' own internals.
 *
 * KNOWN LIMIT: the real `paramsForSave` is a closure inside the ChartExplore component and
 * cannot be imported, so this is a transcription. It defends against `harnessParams` drifting
 * from the descriptors — the failure mode that has actually happened four times — but not
 * against ChartExplore itself changing its save shape. That change would be a v3 params format,
 * which is not a silent edit.
 */
function paramsForSave(viz: VizType, config: unknown, temporalUnits: Record<string, string>) {
  const descriptor = getDescriptor(viz)!;
  return {
    version: 2 as const,
    spec: descriptor.buildSpec(config as never, { ...SHARED, temporalUnits }),
    settings: {
      ...mergeVizSettings({}, descriptor, config as never),
      column_settings: {},
      conditional_formatting: [],
    },
  };
}

const PIE: HarnessChart = {
  viz: "pie",
  config: { dimensions: ["category"], metric: "revenue" },
  columns: COLUMNS,
};

const PIVOT: HarnessChart = {
  viz: "pivot_table",
  config: {
    rows: ["state"], cols: ["category"], metrics: ["revenue"],
    showRowTotals: true, showColTotals: true, condense: true,
  },
  columns: COLUMNS,
};

const TABLE: HarnessChart = {
  viz: "table",
  config: { columns: ["category", "state"], metrics: ["revenue"] },
  columns: COLUMNS,
};

const LINE: HarnessChart = {
  viz: "line",
  config: { xAxis: "created_at", yMetrics: ["revenue"] },
  columns: COLUMNS,
};

describe("harnessParams is byte-identical to the studio's paramsForSave", () => {
  it("matches for a pie — the chart whose settings key the harness got singular", () => {
    expect(harnessParams(PIE)).toEqual(paramsForSave("pie", PIE.config, {}));
    // The historical bug, named: plural, and emitted by the descriptor, not by this test.
    expect(harnessParams(PIE).settings).toHaveProperty("pie.dimensions", ["category"]);
    expect(harnessParams(PIE).settings).not.toHaveProperty("pie.dimension");
  });

  it("matches for a pivot — the chart whose rollup_groups the harness emptied", () => {
    expect(harnessParams(PIVOT)).toEqual(paramsForSave("pivot_table", PIVOT.config, {}));
    // PR 05b: the stored spec had `rollup_groups: []` while the editor rebuilt it from settings.
    expect(harnessParams(PIVOT).spec.rollup_groups).toEqual([["state"], ["category"]]);
  });

  it("matches for a table — the chart the harness saved with settings: {}", () => {
    expect(harnessParams(TABLE)).toEqual(paramsForSave("table", TABLE.config, {}));
    expect(harnessParams(TABLE).settings).toHaveProperty("table.columns", ["category", "state"]);
  });
});

describe("settings is never empty", () => {
  const ALL: HarnessChart[] = [PIE, PIVOT, TABLE, LINE];

  it("carries at least one key for every chart", () => {
    for (const c of ALL) expect(Object.keys(harnessParams(c).settings).length).toBeGreaterThan(0);
  });

  // The strong form of trap 4. `column_settings` and `conditional_formatting` are always
  // written, so "not literally {}" would pass even for the charts that broke — what actually
  // broke them was settings with NO key in the viz's own namespace, which sends the editor
  // back to recovering the config from the spec.
  it("carries at least one key in the viz's own namespace", () => {
    for (const c of ALL) {
      const prefix = getDescriptor(c.viz)!.prefix;
      const own = Object.keys(harnessParams(c).settings).filter((k) => k.startsWith(prefix));
      expect(own, `${c.viz} wrote no ${prefix}* settings`).not.toHaveLength(0);
    }
  });

  it("refuses to emit a chart with no settings in its own namespace", () => {
    // An empty cartesian config produces no `cartesian.*` keys at all — precisely the shape
    // that made three charts open on "Set up your Table". The generator must not hand it back.
    expect(() => harnessParams({ viz: "line", config: {}, columns: COLUMNS })).toThrow(/cartesian\./);
  });
});

describe("a temporal dimension defaults to monthly grouping", () => {
  it("fills temporal_units for a line chart on a date axis", () => {
    expect(harnessParams(LINE).spec.temporal_units).toEqual({ created_at: "month" });
  });

  it("fills temporal_units for a pivot with a date row", () => {
    const p = harnessParams({
      viz: "pivot_table",
      config: { rows: ["created_at"], cols: ["category"], metrics: ["revenue"] },
      columns: COLUMNS,
    });
    expect(p.spec.temporal_units).toEqual({ created_at: "month" });
  });

  it("leaves a non-temporal dimension alone", () => {
    expect(harnessParams(PIE).spec.temporal_units).toEqual({});
  });

  it("does not overwrite an explicit unit", () => {
    const p = harnessParams({ ...LINE, temporalUnits: { created_at: "day_of_week" } });
    expect(p.spec.temporal_units).toEqual({ created_at: "day_of_week" });
  });

  it("still matches paramsForSave once the studio's own effect has run", () => {
    // The studio reaches `{created_at: "month"}` through a useEffect and only then saves; the
    // generator must land on the same params in one pass.
    expect(harnessParams(LINE)).toEqual(paramsForSave("line", LINE.config, { created_at: "month" }));
  });
});

describe("the shell's own settings ride along", () => {
  it("writes column_settings and conditional_formatting", () => {
    const p = harnessParams({
      ...TABLE,
      columnSettings: { revenue: { number_style: "currency" } },
      conditionalFormatting: [
        { columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#f00" },
      ],
    });
    expect(p.settings.column_settings).toEqual({ revenue: { number_style: "currency" } });
    expect(p.settings.conditional_formatting).toHaveLength(1);
  });

  it("carries settings this build does not read straight through", () => {
    const p = harnessParams({ ...PIE, settings: { "some.future.key": 7 } });
    expect(p.settings["some.future.key"]).toBe(7);
  });
});
