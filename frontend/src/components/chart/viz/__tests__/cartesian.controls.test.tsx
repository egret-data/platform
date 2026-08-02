import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { cartesianDescriptor } from "../cartesian";
import type { VizControlProps } from "../types";
import type { CartesianConfig } from "@/api/charts";

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

  it("does not offer max categories to scatter — its x-axis is a metric, not a fold-able category", () => {
    expect(ctrls("scatter").map((c) => c.id)).not.toContain("maxCategories");
    expect(ctrls("bar").map((c) => c.id)).toContain("maxCategories");
  });

  it("does not offer bar's orientation to line or row — row is already the horizontal kind", () => {
    expect(ctrls("bar").map((c) => c.id)).toContain("orientation");
    expect(ctrls("line").map((c) => c.id)).not.toContain("orientation");
    expect(ctrls("row").map((c) => c.id)).not.toContain("orientation");
  });

  it("offers per-series line style to line and area only, grouped under Series and hidden behind advanced", () => {
    for (const k of ["line", "area"] as const) {
      const control = ctrls(k).find((c) => c.id === "seriesLineStyle");
      expect(control, `${k} is missing seriesLineStyle`).toBeTruthy();
      expect(control!.group).toBe("Series");
      expect(control!.advanced).toBe(true);
    }
    for (const k of ["bar", "row", "scatter", "combo"] as const) {
      expect(ctrls(k).map((c) => c.id), `${k} should not offer seriesLineStyle`).not.toContain("seriesLineStyle");
    }
  });

  /**
   * THE DESCRIPTOR -> PANEL SEAM, which is what PR 02 found untested on all three chart families
   * and only a mutation caught. `AxisRangeOptions` greys out the x-label rotations wherever the
   * builder will draw a value axis, and since PR 03 that answer turns on the x column's TYPE — so
   * the type has to travel from the last result, through this descriptor, into the control. Asserted
   * through the descriptor's own `render` rather than by rendering the section directly: a section
   * test passes just as happily when nothing hands the prop over.
   */
  const numericScatterProps: VizControlProps<CartesianConfig> = {
    config: { yMetrics: ["rating"], xAxis: "price" },
    onChange: () => {},
    columns: [], metrics: [],
    resultColumns: [{ name: "price", type: "double" }, { name: "rating", type: "double" }],
    rows: [],
    unitRow: () => null,
  };

  it("hands the x column's result type to the range control, which greys out a dead rotation", async () => {
    const user = userEvent.setup();
    const control = ctrls("scatter").find((c) => c.id === "range");
    render(createElement(control!.render, numericScatterProps));
    await user.click(screen.getByLabelText("X axis lines and ticks"));
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).toHaveAttribute("aria-disabled", "true");
  });

  /** The same seam on the OTHER control that grew the prop. `AxesOptions` prints the RESOLVED scale
   *  on its trigger, so without the type travelling it would read "Ordinal" under a scatter drawing
   *  a value axis — and the two controls are threaded separately, so one being right says nothing
   *  about the other. */
  it("hands it to the axes control too, which prints the resolved scale rather than the stored one", () => {
    const control = ctrls("scatter").find((c) => c.id === "axes");
    render(createElement(control!.render, numericScatterProps));
    expect(screen.getByLabelText("X axis scale")).toHaveTextContent("Linear");
  });

  it("gives every control a unique id, for every kind", () => {
    for (const k of ["bar", "line", "area", "scatter", "combo", "row"] as const) {
      const ids = ctrls(k).map((c) => c.id);
      expect(new Set(ids).size, `${k} has duplicate control ids: ${ids.join(", ")}`).toBe(ids.length);
    }
  });
});
