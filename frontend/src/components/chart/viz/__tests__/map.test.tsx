import { describe, it, expect } from "vitest";
import { mapDescriptor } from "../map";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: {}, orderby: [] };
const d = mapDescriptor();

describe("mapDescriptor", () => {
  it("names both missing fields", () => {
    expect(d.requiredFields({ mapKey: "us_states" })).toEqual(["a region", "a metric"]);
  });

  it("defaults to the us_states map", () => {
    expect(d.defaultConfig.mapKey).toBe("us_states");
  });

  it("builds a spec grouping by the region column", () => {
    const spec = d.buildSpec({ mapKey: "world", region: "country", metric: "total_revenue" }, SHARED);
    expect(spec.dimensions).toEqual(["country"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("reads a v1 map chart from its nested key", () => {
    expect(d.fromParams({ spec: EMPTY_SPEC, map: { mapKey: "world", region: "country", metric: "m" } }))
      .toEqual({ mapKey: "world", region: "country", metric: "m" });
  });

  it("round-trips config → settings → config", () => {
    const cfg = { mapKey: "vietnam", region: "province", metric: "m" };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });
});
