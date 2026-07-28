import { describe, it, expect } from "vitest";
import { cartesianDescriptor } from "../cartesian";

const ctrls = (k: Parameters<typeof cartesianDescriptor>[0]) => cartesianDescriptor(k).controls;

describe("cartesian controls are grouped and per-kind", () => {
  it("groups bar's display controls under Series, Axes and Format", () => {
    const groups = ctrls("bar").filter((c) => c.section === "display").map((c) => c.group);
    expect(new Set(groups)).toEqual(new Set(["Series", "Axes", "Format"]));
  });

  it("puts the rare ones behind advanced", () => {
    const advanced = ctrls("bar").filter((c) => c.advanced).map((c) => c.id);
    expect(advanced).toContain("range");
    expect(advanced).toContain("goal");
  });

  it("offers max categories on the Data tab", () => {
    expect(ctrls("bar").filter((c) => c.section === "data").map((c) => c.id)).toContain("maxCategories");
  });

  it("does not offer bar's orientation to line", () => {
    expect(ctrls("bar").map((c) => c.id)).toContain("orientation");
    expect(ctrls("line").map((c) => c.id)).not.toContain("orientation");
  });

  it("gives every control a unique id, for every kind", () => {
    for (const k of ["bar", "line", "area", "scatter", "combo", "row"] as const) {
      const ids = ctrls(k).map((c) => c.id);
      expect(new Set(ids).size, `${k} has duplicate control ids: ${ids.join(", ")}`).toBe(ids.length);
    }
  });
});
