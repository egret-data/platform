import { describe, it, expect } from "vitest";
import { applyVizOverride } from "../params";
import type { Chart } from "@/api/charts";

const chart = {
  id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "",
  viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [{ col: "x", op: "eq", value: 1 }], orderby: [], row_limit: 100, rollup_groups: [] }, cartesian: { yMetrics: ["rev"], stacked: false } },
} as unknown as Chart;

describe("applyVizOverride", () => {
  it("no override → identity (same viz_type + params)", () => {
    const r = applyVizOverride(chart, undefined);
    expect(r.viz_type).toBe("bar");
    expect(r.params).toEqual(chart.params);
  });
  it("merges display fields and can override viz_type, never touching spec", () => {
    const r = applyVizOverride(chart, { viz_type: "line", cartesian: { stacked: true } } as never);
    expect(r.viz_type).toBe("line");
    expect(r.params.cartesian).toEqual({ yMetrics: ["rev"], stacked: true }); // deep-merged
    expect(r.params.spec).toEqual(chart.params.spec); // query untouched
  });
  it("ignores a spec field in the override", () => {
    const r = applyVizOverride(chart, { spec: { metrics: [] } } as never);
    expect(r.params.spec).toEqual(chart.params.spec);
  });
});
