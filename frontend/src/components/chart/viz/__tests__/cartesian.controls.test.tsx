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
   * THE SAME SEAM, ON THE COLOUR SWATCH — and the whole of the panel-side colour rule, asserted here
   * rather than against the function that computes it.
   *
   * `SeriesListControl` (viz/cartesian.tsx) is where the app decides what a swatch shows, and that
   * decision was covered by NOTHING: the settings-panel tests called the helper through a fixture
   * that REIMPLEMENTED the call, and `SeriesListSection.test.tsx` stubs `defaultColor` with a
   * constant. Mutating the line left the whole of `src/components/chart` green. A helper that
   * restates the line under test is not coverage of it — it is the pre-sorted-fixture mistake one
   * level up — so every case now renders the control the descriptor actually declares and reads the
   * swatch out of the DOM, the way the pie panel's own tests already did.
   */
  describe("the series colour swatch, through the descriptor's own control", () => {
    const cols = [
      { name: "month", type: "varchar" },
      { name: "category", type: "varchar" },
      { name: "revenue", type: "double" },
      { name: "orders", type: "bigint" },
    ];
    /** Breakout order is first-seen and deliberately NOT alphabetical — `colorsForKeys` assigns in
     *  sorted key order, so a pre-sorted fixture cannot tell a by-name lookup from a by-position one.
     *  "Gadget" and "Widget" also hash onto one slot, so this walks the collision path. */
    const rows: unknown[][] = [
      ["2026-01", "Widget", 10, 1], ["2026-01", "Doohickey", 20, 2],
      ["2026-01", "Gizmo", 30, 3], ["2026-01", "Gadget", 40, 4],
      ["2026-02", "Widget", 50, 5], ["2026-02", "Doohickey", 60, 6],
      ["2026-02", "Gizmo", 70, 7], ["2026-02", "Gadget", 80, 8],
    ];
    const GROUPS = ["Widget", "Doohickey", "Gizmo", "Gadget"];
    type Kind = Parameters<typeof cartesianDescriptor>[0];

    const renderSeriesList = (config: CartesianConfig, kind: Kind = "bar") => {
      const control = ctrls(kind).find((c) => c.id === "seriesList");
      render(createElement(control!.render, {
        config, onChange: () => {}, columns: [], metrics: [],
        resultColumns: cols, rows, unitRow: () => null,
      } as VizControlProps<CartesianConfig>));
    };
    /** A native `<input type="color">` normalises its value to lower-case (HTML spec), which is also
     *  what this control writes back — so both sides are compared in that casing. */
    const swatch = (name: string) => (screen.getByLabelText(`color ${name}`) as HTMLInputElement).value.toLowerCase();
    /** Every series the chart draws, keyed by the RAW name the settings key on. `__raw || name`
     *  rather than `__raw ?? name`: an un-broken-out scatter's one group is named "" and is labelled
     *  with its y-metric, which is the key both the builder and this panel use. */
    const chartColors = (config: CartesianConfig, kind: Kind = "bar"): Record<string, string> => {
      const { series } = buildCartesianOption(cols, rows, { kind, ...config }) as unknown as {
        series: { name: string; __raw?: string; itemStyle?: { color?: string }; lineStyle?: { color?: string } }[];
      };
      return Object.fromEntries(series.map((s) =>
        [s.__raw || s.name, (s.itemStyle?.color ?? s.lineStyle?.color ?? "").toLowerCase()]));
    };
    /** Every row's swatch against the colour the chart puts on that series. */
    const expectAgreement = (config: CartesianConfig, expected: string[], kind: Kind = "bar") => {
      const chart = chartColors(config, kind);
      expect(Object.keys(chart)).toEqual(expected); // the unsorted emission order this pins against
      renderSeriesList(config, kind);
      for (const n of expected) expect(swatch(n)).toBe(chart[n]);
      return chart;
    };

    /** The case the defect was live in: the panel showed `#689735` while the chart drew the colour
     *  the metric's name hashes to, so the picker offered a colour that was not on screen. */
    it("shows a lone series its hashed colour, not the summarize green", () => {
      const chart = expectAgreement({ yMetrics: ["revenue"], xAxis: "month" }, ["revenue"]);
      expect(chart.revenue).not.toBe("#689735");
    });

    it("shows two metrics what the chart draws them in", () => {
      expectAgreement({ yMetrics: ["revenue", "orders"], xAxis: "month" }, ["revenue", "orders"]);
    });

    it("shows every breakout band the colour the chart draws it in", () => {
      expectAgreement({ yMetrics: ["revenue"], xAxis: "month", series: "category" }, GROUPS);
    });

    /**
     * A metric-vs-metric scatter has no breakout, so the builder's single group is named "" — and it
     * keys that group on the Y-METRIC instead, precisely because "" is not a name this panel is ever
     * offered. The two have to agree on which key that is, or a pinned colour lands on a series that
     * does not exist.
     */
    it("agrees on an un-broken-out scatter, which the builder keys by its y-metric", () => {
      expectAgreement({ yMetrics: ["revenue"], xMetric: "orders" }, ["revenue"], "scatter");
    });

    it("agrees on a scatter broken out into bands", () => {
      expectAgreement({ yMetrics: ["revenue"], xMetric: "orders", series: "category" }, GROUPS, "scatter");
    });

    it("hands the pinned colours over too, so an override moves the band it displaced", () => {
      // `revenue`'s own ink pinned onto a band instead: `colorsForKeys` CONSUMES it, so whichever
      // band would have hashed onto that slot moves along — in the chart, and so in this panel.
      expectAgreement(
        { yMetrics: ["revenue"], xAxis: "month", series: "category", seriesColors: { Gizmo: "#f2a86f" } },
        GROUPS,
      );
      expect(new Set(GROUPS.map(swatch)).size).toBe(4); // nobody was handed the pinned ink twice
    });

    /**
     * …and every name goes in, hidden ones included — the OPPOSITE of the pie, so it is stated
     * rather than inherited. `applySeriesOrder` drops hidden series AFTER `colorsForKeys` has run,
     * so hiding one does not move the rest; the pie drops hidden slices BEFORE its ring is built, so
     * there they do re-key.
     *
     * Gadget is the band to hide: it wins the Gadget/Widget collision and Widget walks off it, so
     * dropping Gadget from the key set would free that slot and MOVE Widget. Hiding any of the other
     * three leaves everybody where they were and cannot tell the two rules apart.
     */
    it("keeps hidden series in the key set, because the chart colours before it hides", () => {
      const config: CartesianConfig = { yMetrics: ["revenue"], xAxis: "month", series: "category", hiddenSeries: ["Gadget"] };
      const chart = chartColors(config);
      expect(chart.Gadget).toBeUndefined(); // the chart really has dropped it
      renderSeriesList(config);
      for (const n of ["Widget", "Doohickey", "Gizmo"]) expect(swatch(n)).toBe(chart[n]);
      // …and Widget is still where it was before anything was hidden.
      expect(chart.Widget).toBe(chartColors({ ...config, hiddenSeries: [] }).Widget);
    });
  });

  it("gives every control a unique id, for every kind", () => {
    for (const k of ["bar", "line", "area", "scatter", "combo", "row"] as const) {
      const ids = ctrls(k).map((c) => c.id);
      expect(new Set(ids).size, `${k} has duplicate control ids: ${ids.join(", ")}`).toBe(ids.length);
    }
  });
});
