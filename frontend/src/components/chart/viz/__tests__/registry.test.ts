import { describe, it, expect } from "vitest";
import { CARTESIAN_VIZ } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { getDescriptor, hasDescriptor } from "../registry";

describe("vizRegistry", () => {
  it("has a descriptor for every cartesian kind in PR1", () => {
    for (const k of CARTESIAN_VIZ) expect(hasDescriptor(k)).toBe(true);
  });

  it("has a descriptor for the four PR2 viz types", () => {
    for (const k of ["table", "pivot_table", "object", "map"] as const) expect(hasDescriptor(k)).toBe(true);
  });

  it("has a descriptor for every viz type — the bridge is gone", () => {
    for (const t of CHART_TYPES) expect(hasDescriptor(t.key)).toBe(true);
  });

  it("returns a descriptor whose key matches the lookup", () => {
    expect(getDescriptor("bar")?.key).toBe("bar");
  });

  it("gives each cartesian kind its own meta", () => {
    expect(getDescriptor("bar")?.meta.name).not.toBe(getDescriptor("line")?.meta.name);
  });
});
