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
