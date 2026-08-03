import { describe, it, expect } from "vitest";
import { cartesianFromParams, cartesianFromSettings, cartesianToSettings } from "../cartesian-codec";
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

  // Same failure mode as the table's: a v2 row that never got a `cartesian.*` key used to open
  // with no metric at all, though the spec it was saved with names one.
  it("falls back to the spec metrics when a v2 chart has no cartesian key", () => {
    const cfg = cartesianFromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, metrics: ["total_revenue"] },
      settings: { "pie.metric": "someone else's key" },
    });
    expect(cfg).toEqual({ yMetrics: ["total_revenue"] });
  });

  it("prefers the stored metrics over the spec", () => {
    const cfg = cartesianFromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, metrics: ["from_spec"] },
      settings: { "cartesian.yMetrics": ["stored"] },
    });
    expect(cfg).toEqual({ yMetrics: ["stored"] });
  });

  it("round-trips config → settings → config", () => {
    expect(cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(CFG) })).toEqual(CFG);
  });

  it("round-trips maxCategories and compactValues", () => {
    const cfg: CartesianConfig = { yMetrics: ["total_revenue"], maxCategories: 8, compactValues: true };
    expect(cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(cfg) })).toEqual(cfg);
  });

  it("round-trips seriesOrder and hiddenSeries", () => {
    const cfg: CartesianConfig = {
      yMetrics: ["total_revenue", "order_count"],
      seriesOrder: ["order_count", "total_revenue"],
      hiddenSeries: ["total_revenue"],
    };
    expect(cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(cfg) })).toEqual(cfg);
  });

  /**
   * `KEYS` is `as const satisfies readonly (keyof CartesianConfig)[]`, which checks that every key
   * listed EXISTS and says nothing about the ones missing — so a new setting left out of it
   * typechecks, renders, redraws the chart, and is gone the next time the chart is opened. Saving
   * and reloading is the only thing that catches it, so each setting that reaches the panel gets a
   * round-trip here.
   */
  it("round-trips xLabelRotation, which nothing but a reload can prove", () => {
    const cfg: CartesianConfig = { yMetrics: ["total_revenue"], xLabelRotation: 45, xAxisTicks: false };
    expect(cartesianToSettings(cfg)["cartesian.xLabelRotation"]).toBe(45);
    expect(cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(cfg) })).toEqual(cfg);
  });

  it("round-trips labelDensity", () => {
    const cfg: CartesianConfig = { yMetrics: ["total_revenue"], labelDensity: "all" };
    expect(cartesianFromParams({ version: 2, spec: EMPTY_SPEC, settings: cartesianToSettings(cfg) })).toEqual(cfg);
  });

  it("round-trips the line settings", () => {
    const cfg = {
      yMetrics: ["rev"],
      lineShape: "step" as const,
      missing: "zero" as const,
      markers: "off" as const,
      seriesLine: { rev: { style: "dashed" as const, size: "L" as const } },
    };
    expect(cartesianFromSettings(cartesianToSettings(cfg))).toEqual(cfg);
  });

  it("no longer carries the booleans the line settings replaced", () => {
    const settings = cartesianToSettings({
      yMetrics: [], lineShape: "linear",
    } as never);
    for (const dead of ["cartesian.smooth", "cartesian.step", "cartesian.connectNulls", "cartesian.showMarkers"]) {
      expect(Object.keys(settings)).not.toContain(dead);
    }
  });
});
