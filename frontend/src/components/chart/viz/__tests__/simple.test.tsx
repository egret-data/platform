import { describe, it, expect } from "vitest";
import { SIMPLE_DESCRIPTORS } from "../simple";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };
const byKey = Object.fromEntries(SIMPLE_DESCRIPTORS.map((d) => [d.key, d]));

describe("the nine simple descriptors", () => {
  it("covers exactly the viz types PR3 migrates", () => {
    expect(SIMPLE_DESCRIPTORS.map((d) => d.key).sort()).toEqual(
      ["boxplot", "funnel", "gauge", "pie", "progress", "sankey", "scalar", "smartscalar", "waterfall"],
    );
  });

  it("keeps pie's required labels", () => {
    expect(byKey.pie.requiredFields({})).toEqual(["a dimension", "a metric"]);
  });

  it("keeps scalar's single-row spec", () => {
    const spec = byKey.scalar.buildSpec({ metric: "m" }, SHARED);
    expect(spec.metrics).toEqual(["m"]);
    expect(spec.row_limit).toBe(1);
  });

  it("keeps smartscalar's ordering by its dimension", () => {
    const spec = byKey.smartscalar.buildSpec({ dimension: "order_date", metric: "m" }, SHARED);
    expect(spec.orderby).toEqual([{ field: "order_date", desc: false }]);
  });

  it("keeps boxplot on raw columns", () => {
    const spec = byKey.boxplot.buildSpec({ category: "region", value: "revenue" }, SHARED);
    expect(spec.columns).toEqual(["region", "revenue"]);
    expect(spec.metrics).toEqual([]);
    expect(spec.dimensions).toEqual([]);
  });

  it("keeps sankey's two dimensions in source→target order", () => {
    const spec = byKey.sankey.buildSpec({ source: "a", target: "b", metric: "m" }, SHARED);
    expect(spec.dimensions).toEqual(["a", "b"]);
  });

  it("prunes temporal units to the grouped dimension", () => {
    expect(byKey.pie.buildSpec({ dimension: "order_date", metric: "m" }, SHARED).temporal_units)
      .toEqual({ order_date: "month" });
    expect(byKey.pie.buildSpec({ dimension: "region", metric: "m" }, SHARED).temporal_units).toEqual({});
  });

  it("round-trips every descriptor's config through settings", () => {
    const sample: Record<string, Record<string, unknown>> = {
      pie: { dimension: "region", metric: "m" },
      scalar: { metric: "m" },
      progress: { metric: "m", goal: 10 },
      gauge: { metric: "m", min: 0, max: 100 },
      funnel: { dimension: "region", metric: "m" },
      waterfall: { xAxis: "region", metric: "m", showTotal: true },
      smartscalar: { dimension: "order_date", metric: "m" },
      boxplot: { category: "region", value: "revenue" },
      sankey: { source: "a", target: "b", metric: "m" },
    };
    for (const d of SIMPLE_DESCRIPTORS) {
      const cfg = sample[d.key];
      expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
    }
  });

  it("reads every descriptor's v1 nested key", () => {
    expect(byKey.waterfall.fromParams({ spec: EMPTY_SPEC, waterfall: { xAxis: "region", metric: "m" } }))
      .toEqual({ xAxis: "region", metric: "m" });
    expect(byKey.boxplot.fromParams({ spec: EMPTY_SPEC, boxplot: { category: "c", value: "v" } }))
      .toEqual({ category: "c", value: "v" });
  });

  it("reads a pre-nested-key v1 chart from its spec", () => {
    expect(byKey.scalar.fromParams({ spec: { ...EMPTY_SPEC, metrics: ["m"] } })).toEqual({ metric: "m" });
    expect(byKey.boxplot.fromParams({ spec: { ...EMPTY_SPEC, columns: ["c", "v"] } }))
      .toEqual({ category: "c", value: "v" });
  });

  it("gives pie a display control and the others none", () => {
    expect(byKey.pie.controls.some((c) => c.section === "display")).toBe(true);
    expect(byKey.scalar.controls.every((c) => c.section === "data")).toBe(true);
  });
});
