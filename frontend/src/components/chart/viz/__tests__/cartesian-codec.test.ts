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
