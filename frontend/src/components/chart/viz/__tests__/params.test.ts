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
