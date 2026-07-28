import { describe, it, expect } from "vitest";
import { applyVizOverride, flattenLegacyOverride } from "../params";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, Chart } from "@/api/charts";

const SPEC = {
  metrics: ["rev"], dimensions: [], columns: [],
  filters: [{ col: "x", op: "eq", value: 1 }], orderby: [], row_limit: 100, rollup_groups: [],
};

const chart = {
  id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "",
  viz_type: "bar",
  params: {
    version: 2, spec: SPEC,
    settings: { "cartesian.yMetrics": ["rev"], "cartesian.stacked": false },
  },
} as unknown as Chart;

/** A chart saved before the v2 backfill. */
const v1Chart = {
  ...chart,
  params: { spec: SPEC, cartesian: { yMetrics: ["rev"], stacked: false } },
} as unknown as Chart;

const settingsOf = (p: AnyChartParams) => (isParamsV2(p) ? p.settings : {});

describe("applyVizOverride", () => {
  it("no override → identity", () => {
    const r = applyVizOverride(chart, undefined);
    expect(r.viz_type).toBe("bar");
    expect(r.params).toEqual(chart.params);
  });

  it("merges a settings partial by key and can override viz_type, never touching spec", () => {
    const r = applyVizOverride(chart, { viz_type: "line", "cartesian.stacked": true });
    expect(r.viz_type).toBe("line");
    expect(settingsOf(r.params)).toEqual({ "cartesian.yMetrics": ["rev"], "cartesian.stacked": true });
    expect(r.params.spec).toEqual(SPEC);
  });

  it("ignores a spec field in the override", () => {
    const r = applyVizOverride(chart, { spec: { metrics: [] } });
    expect(r.params.spec).toEqual(SPEC);
  });

  it("still accepts a v1-shaped override stored before migration 0100", () => {
    const r = applyVizOverride(chart, {
      cartesian: { stacked: true },
      columnSettings: { rev: { number_style: "percent" } },
    });
    expect(settingsOf(r.params)).toEqual({
      "cartesian.yMetrics": ["rev"],
      "cartesian.stacked": true,
      column_settings: { rev: { number_style: "percent" } },
    });
  });

  it("lifts a v1 chart's params to v2 before merging", () => {
    const r = applyVizOverride(v1Chart, { "cartesian.stacked": true });
    expect(settingsOf(r.params)).toEqual({ "cartesian.stacked": true });
    expect(r.params.spec).toEqual(SPEC);
  });
});

describe("flattenLegacyOverride", () => {
  it("flattens nested config objects", () => {
    expect(flattenLegacyOverride({ pie: { donut: true, showLegend: false } }))
      .toEqual({ "pie.donut": true, "pie.showLegend": false });
  });

  it("renames the shell-owned keys", () => {
    expect(flattenLegacyOverride({ columnSettings: {}, conditionalFormatting: [], tableState: { sort: "a" } }))
      .toEqual({ column_settings: {}, conditional_formatting: [], "table.state": { sort: "a" } });
  });

  it("renames the map's key field", () => {
    expect(flattenLegacyOverride({ map: { mapKey: "world", region: "country" } }))
      .toEqual({ "map.key": "world", "map.region": "country" });
  });

  it("passes an already-flat override through untouched", () => {
    const flat = { "cartesian.stacked": true, column_settings: { a: {} } };
    expect(flattenLegacyOverride(flat)).toEqual(flat);
  });
});
