import { describe, it, expect } from "vitest";
import { missingFields } from "../requiredFields";

describe("missingFields", () => {
  it("bar needs an X-axis and a metric", () => {
    expect(missingFields("bar", { cartesian: { yMetrics: [] } })).toEqual(["X-axis", "a metric"]);
    expect(missingFields("bar", { cartesian: { xAxis: "region", yMetrics: ["rev"] } })).toEqual([]);
  });
  it("pie needs a dimension and a metric", () => {
    expect(missingFields("pie", { pie: {} })).toEqual(["a dimension", "a metric"]);
  });
  it("big number needs a metric", () => {
    expect(missingFields("scalar", { scalar: {} })).toEqual(["a metric"]);
    expect(missingFields("scalar", { scalar: { metric: "rev" } })).toEqual([]);
  });
  it("progress needs a metric", () => {
    expect(missingFields("progress", { progress: {} })).toEqual(["a metric"]);
  });
  it("gauge needs a metric", () => {
    expect(missingFields("gauge", { gauge: {} })).toEqual(["a metric"]);
  });
  it("funnel needs a dimension and a metric", () => {
    expect(missingFields("funnel", { funnel: {} })).toEqual(["a dimension", "a metric"]);
  });
  it("waterfall needs an x-axis and a metric", () => {
    expect(missingFields("waterfall", { waterfall: {} })).toEqual(["an x-axis", "a metric"]);
  });
  it("smartscalar needs a time dimension and a metric", () => {
    expect(missingFields("smartscalar", { smart: {} })).toEqual(["a time dimension", "a metric"]);
  });
  it("boxplot needs a category and a value column", () => {
    expect(missingFields("boxplot", { box: {} })).toEqual(["a category", "a value column"]);
  });
  it("sankey needs a source, a target, and a metric", () => {
    expect(missingFields("sankey", { sankey: {} })).toEqual(["a source", "a target", "a metric"]);
  });
  it("pivot_table needs a row dimension and a value", () => {
    expect(missingFields("pivot_table", { pivot: { rows: [], vals: [] } })).toEqual(["a row dimension", "a value"]);
  });
  it("object needs at least one column", () => {
    expect(missingFields("object", { objectCols: [] })).toEqual(["at least one column"]);
  });
  it("returns [] for map once region and metric are set", () => {
    expect(missingFields("map", { map: { region: "r", metric: "m" } })).toEqual([]);
  });
  it("returns [] for a viz with no requirement branch (table)", () => {
    expect(missingFields("table", {})).toEqual([]);
  });
});
