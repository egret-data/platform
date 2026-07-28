import { describe, it, expect } from "vitest";
import { fieldDescriptor } from "../fields";
import { EMPTY_SPEC } from "@/api/charts";

const d = fieldDescriptor({
  key: "funnel",
  prefix: "funnel.",
  fields: [
    { id: "dimension", label: "Stage (dimension)", kind: "dimension", required: "a dimension", temporal: true },
    { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
  ],
  buildSpec: (cfg, shared, helpers) => helpers.dimsAndMetrics([cfg.dimension], [cfg.metric], shared),
  legacyKey: "funnel",
  legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
});

describe("fieldDescriptor", () => {
  it("derives requiredFields from the field list, in order", () => {
    expect(d.requiredFields({})).toEqual(["a dimension", "a metric"]);
    expect(d.requiredFields({ dimension: "region" })).toEqual(["a metric"]);
    expect(d.requiredFields({ dimension: "region", metric: "m" })).toEqual([]);
  });

  it("writes namespaced settings and reads them back", () => {
    const cfg = { dimension: "region", metric: "m" };
    expect(d.toSettings(cfg)).toEqual({ "funnel.dimension": "region", "funnel.metric": "m" });
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  it("reads a v1 chart from its nested key", () => {
    expect(d.fromParams({ spec: EMPTY_SPEC, funnel: { dimension: "region", metric: "m" } }))
      .toEqual({ dimension: "region", metric: "m" });
  });

  it("falls back to the v1 spec when the nested key is missing", () => {
    const cfg = d.fromParams({ spec: { ...EMPTY_SPEC, dimensions: ["region"], metrics: ["m"] } });
    expect(cfg).toEqual({ dimension: "region", metric: "m" });
  });

  it("builds a spec from the declared fields", () => {
    const spec = d.buildSpec({ dimension: "region", metric: "m" }, { filters: [], rowLimit: 500, temporalUnits: {} });
    expect(spec.dimensions).toEqual(["region"]);
    expect(spec.metrics).toEqual(["m"]);
    expect(spec.row_limit).toBe(500);
  });

  it("drops an unset field from the spec instead of emitting an empty string", () => {
    const spec = d.buildSpec({ dimension: "region" }, { filters: [], rowLimit: 100, temporalUnits: {} });
    expect(spec.metrics).toEqual([]);
  });

  it("gives every declared field a data control", () => {
    expect(d.controls.map((c) => c.id)).toEqual(["dimension", "metric"]);
    expect(d.controls.every((c) => c.section === "data")).toBe(true);
  });
});
