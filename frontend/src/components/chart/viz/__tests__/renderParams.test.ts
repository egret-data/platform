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

  it("un-flattens the pivot keys back to v1's loose top-level fields", () => {
    const out = toRenderParams("pivot_table", {
      version: 2, spec: SPEC,
      settings: {
        "pivot.rows": ["region"], "pivot.cols": [], "pivot.metrics": ["m"],
        "pivot.showRowTotals": false, "pivot.collapsedRows": ["x"], "pivot.sort": { region: "asc" },
      },
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

  it("omits the cartesian key for a non-cartesian viz", () => {
    const out = toRenderParams("pie", { version: 2, spec: SPEC, settings: { "cartesian.xAxis": "region" } });
    expect(out.cartesian).toBeUndefined();
  });
});
