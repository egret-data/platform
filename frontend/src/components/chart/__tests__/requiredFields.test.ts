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
  it("returns [] for a viz with no requirements yet unknown", () => {
    expect(missingFields("map", { map: { region: "r", metric: "m" } })).toEqual([]);
  });
});
