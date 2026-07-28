import { describe, it, expect } from "vitest";
import { cartesianDescriptor } from "../cartesian";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };

describe("cartesianDescriptor", () => {
  it("names the missing fields of an empty config", () => {
    expect(cartesianDescriptor("bar").requiredFields({ yMetrics: [] })).toEqual(["X-axis", "a metric"]);
  });

  it("reports nothing missing once an axis and a metric are set", () => {
    expect(cartesianDescriptor("bar").requiredFields({ xAxis: "region", yMetrics: ["total_revenue"] })).toEqual([]);
  });

  it("builds a spec with the x-axis and series as dimensions", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], series: "category" },
      SHARED,
    );
    expect(spec.dimensions).toEqual(["region", "category"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
    expect(spec.row_limit).toBe(1000);
  });

  it("prunes temporal units down to the dimensions actually grouped", () => {
    const spec = cartesianDescriptor("line").buildSpec(
      { xAxis: "order_date", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.temporal_units).toEqual({ order_date: "month" });
    const other = cartesianDescriptor("line").buildSpec({ xAxis: "region", yMetrics: ["m"] }, SHARED);
    expect(other.temporal_units).toEqual({});
  });

  it("puts scatter's x-metric first in the metric list", () => {
    const spec = cartesianDescriptor("scatter").buildSpec(
      { xAxis: "region", xMetric: "order_count", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["order_count", "total_revenue"]);
  });

  it("ignores an x-metric for a non-scatter kind", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", xMetric: "order_count", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("round-trips a v1 params object into settings the descriptor reads back", () => {
    const d = cartesianDescriptor("bar");
    const cfg = d.fromParams({ spec: EMPTY_SPEC, cartesian: { xAxis: "region", yMetrics: ["m"], stacked: true } });
    const back = d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) });
    expect(back).toEqual(cfg);
  });

  it("exposes data controls before display controls", () => {
    const sections = cartesianDescriptor("bar").controls.map((c) => c.section);
    expect(sections.indexOf("data")).toBeLessThan(sections.indexOf("display"));
  });

  it("gives scatter an extra x-metric control the other kinds do not have", () => {
    const ids = (k: "bar" | "scatter") => cartesianDescriptor(k).controls.map((c) => c.id);
    expect(ids("scatter")).toContain("xMetric");
    expect(ids("bar")).not.toContain("xMetric");
  });
});
