import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { cartesianDescriptor } from "../cartesian";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
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

  /**
   * THE SAME SEAM, ON THE COLOUR SWATCH — and the one place `defaultSeriesColor` is called in the
   * running app (`SeriesListControl`, viz/cartesian.tsx).
   *
   * That call was covered by NOTHING. `CartesianDisplaySection.test.tsx` exercises the function
   * directly through a helper that REIMPLEMENTS the call; `SeriesListSection.test.tsx` stubs
   * `defaultColor` with a constant. Mutating the line to `defaultSeriesColor([...names].sort(), i)`
   * — which drops `seriesColors` and mis-indexes every breakout — left the whole of
   * `src/components/chart` green. A helper that restates the line under test is not coverage of it;
   * it is the pre-sorted-fixture mistake one level up.
   *
   * So these render the control the descriptor actually declares and read the swatch out of the DOM,
   * exactly as the pie panel's own tests do.
   */
  describe("the series colour swatch, through the descriptor's own control", () => {
    const cols = [
      { name: "month", type: "varchar" },
      { name: "category", type: "varchar" },
      { name: "revenue", type: "double" },
    ];
    /** Breakout order is first-seen and deliberately NOT alphabetical — `colorsForKeys` assigns in
     *  sorted key order, so a pre-sorted fixture cannot tell a by-name lookup from a by-position one.
     *  "Gadget" and "Widget" also hash onto one slot, so this walks the collision path. */
    const rows: unknown[][] = [
      ["2026-01", "Widget", 10], ["2026-01", "Doohickey", 20],
      ["2026-01", "Gizmo", 30], ["2026-01", "Gadget", 40],
    ];
    const GROUPS = ["Widget", "Doohickey", "Gizmo", "Gadget"];

    const renderSeriesList = (config: CartesianConfig) => {
      const control = ctrls("bar").find((c) => c.id === "seriesList");
      render(createElement(control!.render, {
        config, onChange: () => {}, columns: [], metrics: [],
        resultColumns: cols, rows, unitRow: () => null,
      } as VizControlProps<CartesianConfig>));
    };
    /** A native `<input type="color">` normalises its value to lower-case (HTML spec), which is also
     *  what this control writes back — so both sides are compared in that casing. */
    const swatch = (name: string) => (screen.getByLabelText(`color ${name}`) as HTMLInputElement).value.toLowerCase();
    const chartColors = (config: CartesianConfig): Record<string, string> => {
      const { series } = buildCartesianOption(cols, rows, { kind: "bar", ...config }) as unknown as {
        series: { name: string; __raw?: string; itemStyle?: { color?: string } }[];
      };
      return Object.fromEntries(series.map((s) => [s.__raw || s.name, (s.itemStyle?.color ?? "").toLowerCase()]));
    };

    it("shows every breakout band the colour the chart draws it in", () => {
      const config: CartesianConfig = { yMetrics: ["revenue"], xAxis: "month", series: "category" };
      const chart = chartColors(config);
      expect(Object.keys(chart)).toEqual(GROUPS); // the unsorted emission order this pins against
      renderSeriesList(config);
      for (const g of GROUPS) expect(swatch(g)).toBe(chart[g]);
    });

    it("hands the pinned colours over too, so an override moves the band it displaced", () => {
      // `revenue`'s own ink pinned onto a band instead: `colorsForKeys` CONSUMES it, so whichever
      // band would have hashed onto that slot moves along — in the chart, and so in this panel.
      const config: CartesianConfig = {
        yMetrics: ["revenue"], xAxis: "month", series: "category",
        seriesColors: { Gizmo: "#f2a86f" },
      };
      const chart = chartColors(config);
      renderSeriesList(config);
      for (const g of GROUPS) expect(swatch(g)).toBe(chart[g]);
      expect(new Set(GROUPS.map(swatch)).size).toBe(4); // nobody was handed the pinned ink twice
    });
  });

  it("gives every control a unique id, for every kind", () => {
    for (const k of ["bar", "line", "area", "scatter", "combo", "row"] as const) {
      const ids = ctrls(k).map((c) => c.id);
      expect(new Set(ids).size, `${k} has duplicate control ids: ${ids.join(", ")}`).toBe(ids.length);
    }
  });
});
