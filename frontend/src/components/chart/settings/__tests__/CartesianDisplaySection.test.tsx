import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxisRangeOptions, CartesianDisplaySection } from "../CartesianDisplaySection";
import { ChartAxesSection } from "../ChartAxesSection";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
import type { CartesianConfig } from "@/api/charts";

describe("CartesianDisplaySection", () => {
  // Legend and "Shorten large numbers" both default from the data at render time (see
  // buildCartesianOption), so they are a three-way Auto/On/Off select rather than a Switch —
  // `undefined` is a real, visible state, not "off". Each interaction starts from a value
  // that differs from the target, since a Radix Select does not fire onValueChange when the
  // already-selected item is re-clicked.
  it("Legend: choosing On emits true", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Legend"));
    await user.click(await screen.findByRole("option", { name: "On" }));
    expect(onChange).toHaveBeenCalledWith({ showLegend: true });
  });

  it("Legend: choosing Off emits false", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Legend"));
    await user.click(await screen.findByRole("option", { name: "Off" }));
    expect(onChange).toHaveBeenCalledWith({ showLegend: false });
  });

  it("Legend: choosing Auto emits undefined", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], showLegend: true }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Legend"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenCalledWith({ showLegend: undefined });
  });

  it("Legend: shows Auto selected when the config leaves it unset", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(within(screen.getByLabelText("Legend")).getByText("Auto")).toBeInTheDocument();
  });

  it("Shorten large numbers: choosing On emits true", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Shorten large numbers"));
    await user.click(await screen.findByRole("option", { name: "On" }));
    expect(onChange).toHaveBeenCalledWith({ compactValues: true });
  });

  it("Shorten large numbers: choosing Off emits false", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Shorten large numbers"));
    await user.click(await screen.findByRole("option", { name: "Off" }));
    expect(onChange).toHaveBeenCalledWith({ compactValues: false });
  });

  it("Shorten large numbers: choosing Auto emits undefined", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], compactValues: true }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Shorten large numbers"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenCalledWith({ compactValues: undefined });
  });

  it("Shorten large numbers: shows Auto selected when the config leaves it unset", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(within(screen.getByLabelText("Shorten large numbers")).getByText("Auto")).toBeInTheDocument();
  });

  it("Horizontal shows for bar, hides for line", () => {
    const { rerender } = render(<CartesianDisplaySection kind="bar" config={{ yMetrics: [] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Horizontal")).toBeInTheDocument();
    rerender(<CartesianDisplaySection kind="line" config={{ yMetrics: [] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Horizontal")).not.toBeInTheDocument();
  });
  // The per-series colour and rename moved to SeriesListSection, which now carries all four
  // per-series settings on one row — see that file's own tests.

  it("offers Metabase's axis line/tick selects and the value-axis tick count", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    // Unset means shown, so a chart saved before these existed keeps drawing both. BOTH axes are a
    // select now — the category axis carries the rotation too, so it has four choices to the value
    // axis' two. The value axis WAS a switch, which is the divergence from the waterfall/box-plot
    // tab the audit measured; driving it is the block at the bottom of this file.
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Show");
    expect(screen.getByLabelText("Y show lines and tick marks")).toHaveTextContent("Show");
    fireEvent.change(screen.getByLabelText("Number of tick marks"), { target: { value: "5" } });
    expect(onChange).toHaveBeenLastCalledWith({ yTickCount: 5 });
  });

  it("clearing the tick count returns the axis to auto", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yTickCount: 5 }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Number of tick marks"), { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith({ yTickCount: undefined });
  });
  /**
   * Metabase's `getHidden` on `graph.y_axis.split_number` — "Number of tick marks" — is
   * `settings["graph.y_axis.axis_enabled"] === false`. The lines-and-ticks toggle is egret's half of
   * that key, so with it off there are no tick marks left for this number to divide. The
   * waterfall/box-plot tab ships the same setting and gets the same rule (ChartAxesSection.test.tsx);
   * fixing one of two copies is how the last defect here came to live in both files.
   *
   * Both directions, because a fixture with the axis on cannot show the box hides.
   */
  it("hides the value-axis tick count once that axis' lines and ticks are off", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yAxisTicks: false }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Number of tick marks")).toBeNull();
  });

  it("…and offers it while they are drawn", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Number of tick marks")).toBeInTheDocument();
  });

  it("Data label density is hidden when data labels are off", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Data label density")).not.toBeInTheDocument();
  });

  it("Data label density: choosing All emits \"all\"", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], showLabels: true }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Data label density"));
    await user.click(await screen.findByRole("option", { name: "All" }));
    expect(onChange).toHaveBeenCalledWith({ labelDensity: "all" });
  });

  it("Data label density: choosing Fit emits \"fit\"", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], showLabels: true, labelDensity: "all" }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Data label density"));
    await user.click(await screen.findByRole("option", { name: "Fit" }));
    expect(onChange).toHaveBeenCalledWith({ labelDensity: "fit" });
  });

  it("Data label density: shows Fit selected when the config leaves it unset", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], showLabels: true }} onChange={() => {}} />);
    expect(within(screen.getByLabelText("Data label density")).getByText("Fit")).toBeInTheDocument();
  });

  /** A stored bound in the fixture, which is also what opens the pair now that it sits behind the
   *  automatic range — see the vocabulary block at the bottom of this file for that gate. */
  it("Y min/max inputs patch yMin/yMax (undefined when cleared)", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yMax: 50 }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Y min"), { target: { value: "0" } });
    expect(onChange).toHaveBeenCalledWith({ yMin: 0 });
    fireEvent.change(screen.getByLabelText("Y max"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ yMax: undefined });
  });

  // Line shape, missing-value and marker controls. The gate was `isLineish` (line/area by KIND)
  // until the audit measured a combo honouring all three while offering none of them; it is now
  // "does any series resolve to a line", the same rule #408 uses to decide which series rows get
  // the per-series drill-in. Shape and markers also have that per-series door; `missing` has only
  // this one, which is why the gate being wrong made it unreachable rather than merely awkward.
  describe("line shape / missing values / markers (any series drawn as a line)", () => {
    it("all three render for line", () => {
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Line shape")).toBeInTheDocument();
      expect(screen.getByLabelText("Missing values")).toBeInTheDocument();
      expect(screen.getByLabelText("Markers")).toBeInTheDocument();
    });

    /**
     * A COMBO REACHES THEM TOO, because `decorateSeries` applies all three to any series whose
     * ECharts type is "line" — and a fresh combo draws its first metric as one.
     *
     * Measured on `kind: "combo"` with `missing: "zero"`, `lineShape: "smooth"`, `markers: "off"`:
     * the line band came back `data: [1, 0, 3]` (the null rewritten to 0), `smooth: true`,
     * `showSymbol: false`. All three took effect and the panel offered none of them.
     *
     * `missing` is the one that matters most: `LineSeriesStyle` deliberately omits it (the argument
     * is on `decorateSeries`), so the per-series popover #408 added cannot reach it either — the
     * chart-wide control was its ONLY door and that door was shut for a combo. And it does not
     * merely restyle: `missing: "zero"` rewrites the data, so it silently moves stack totals,
     * normalisation and any trend fit.
     */
    it("all three render for a combo, whose first metric is drawn as a line", () => {
      render(<CartesianDisplaySection kind="combo" config={{ yMetrics: ["rev", "cnt"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Line shape")).toBeInTheDocument();
      expect(screen.getByLabelText("Missing values")).toBeInTheDocument();
      expect(screen.getByLabelText("Markers")).toBeInTheDocument();
    });

    /**
     * …AND STOP RENDERING ONCE NO SERIES IS A LINE. "Combo is line-ish" would have been the cheap
     * gate and it is wrong here: with every metric flipped to Bar the chart draws no line, so
     * `decorateSeries` applies none of these and the controls would be three settings that change
     * nothing. The honest question is "does any series resolve to a line", which is the same rule
     * #408 used to decide which series rows get the drill-in button.
     *
     * THREE SERIES with the flipped one in the MIDDLE, and the first one flipped too: the gate is a
     * `.some()` over the resolved displays, so a fixture that left the positional default in place
     * anywhere would still find a line and pass whatever the gate said.
     */
    it("none of the three render for a combo whose every series is a bar", () => {
      render(<CartesianDisplaySection kind="combo"
        config={{ yMetrics: ["rev", "cnt", "qty"], seriesTypes: { rev: "bar", cnt: "bar", qty: "bar" } }}
        onChange={() => {}} />);
      expect(screen.queryByLabelText("Line shape")).not.toBeInTheDocument();
      expect(screen.queryByLabelText("Missing values")).not.toBeInTheDocument();
      expect(screen.queryByLabelText("Markers")).not.toBeInTheDocument();
    });

    /** One line among bars is still a line, and the middle one is chosen so the answer cannot come
     *  from the positional default at either end. */
    it("renders them for a combo with a single line band among bars", () => {
      render(<CartesianDisplaySection kind="combo"
        config={{ yMetrics: ["rev", "cnt", "qty"], seriesTypes: { rev: "bar", cnt: "line", qty: "bar" } }}
        onChange={() => {}} />);
      expect(screen.getByLabelText("Line shape")).toBeInTheDocument();
    });

    /**
     * WITH A BREAKOUT the bands are named by the DATA, so the gate reads the names the caller hands
     * in rather than the metrics — the same list the Stacking control above already counts.
     *
     * The first arm is what pins that: the single metric is `rev`, which no `seriesTypes` entry
     * names, so a gate reading the METRICS would resolve it through the positional default to
     * `line` at index 0 and show the controls. It reads the two bar bands instead and hides them.
     * The second arm brings a line band in, in the MIDDLE, where neither end of the positional
     * default can account for the answer.
     */
    it("reads the breakout's band names, not the metrics, when the caller supplies them", () => {
      const config = {
        yMetrics: ["rev"], series: "product",
        seriesTypes: { Widget: "bar", Gadget: "bar", Gizmo: "line" },
      } as CartesianConfig;
      const { rerender } = render(
        <CartesianDisplaySection kind="combo" config={config} onChange={() => {}} seriesNames={["Widget", "Gadget"]} />,
      );
      expect(screen.queryByLabelText("Line shape")).not.toBeInTheDocument();
      rerender(
        <CartesianDisplaySection kind="combo" config={config} onChange={() => {}} seriesNames={["Widget", "Gizmo", "Gadget"]} />,
      );
      expect(screen.getByLabelText("Line shape")).toBeInTheDocument();
    });

    /**
     * BEFORE A QUERY HAS RUN there may be no series to ask about — `cartesianSeriesNames` returns
     * `[]` for a breakout whose result has not arrived. Hiding the controls then would make them
     * flicker in as the query lands; the fallback asks the builder what series 0 would be, which is
     * a line for a combo and for a line/area chart and a bar for the rest.
     */
    it("falls back to the kind's own first-series answer when no series are known yet", () => {
      const { rerender } = render(
        <CartesianDisplaySection kind="combo" config={{ yMetrics: [], series: "product" }} onChange={() => {}} seriesNames={[]} />,
      );
      expect(screen.getByLabelText("Line shape")).toBeInTheDocument();
      rerender(
        <CartesianDisplaySection kind="bar" config={{ yMetrics: [], series: "product" }} onChange={() => {}} seriesNames={[]} />,
      );
      expect(screen.queryByLabelText("Line shape")).not.toBeInTheDocument();
    });

    it("none of the three render for bar", () => {
      render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
      expect(screen.queryByLabelText("Line shape")).not.toBeInTheDocument();
      expect(screen.queryByLabelText("Missing values")).not.toBeInTheDocument();
      expect(screen.queryByLabelText("Markers")).not.toBeInTheDocument();
    });

    it("choosing a line shape emits lineShape", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Line shape"));
      await user.click(await screen.findByRole("option", { name: "Step" }));
      expect(onChange).toHaveBeenCalledWith({ lineShape: "step" });
    });

    it("choosing a missing-value mode emits missing", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Missing values"));
      await user.click(await screen.findByRole("option", { name: "Zero" }));
      expect(onChange).toHaveBeenCalledWith({ missing: "zero" });
    });

    it("Markers: choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], markers: "on" }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Markers"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ markers: undefined });
    });

    it("Markers: choosing On emits \"on\"", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Markers"));
      await user.click(await screen.findByRole("option", { name: "On" }));
      expect(onChange).toHaveBeenCalledWith({ markers: "on" });
    });

    it("Markers: choosing Off emits \"off\"", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Markers"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ markers: "off" });
    });

    it("Markers: an unset value shows Auto selected, not On or Off", () => {
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Markers")).getByText("Auto")).toBeInTheDocument();
    });

    // The regression a plain string Select must not repeat: AutoToggle translates through a
    // boolean, so a stored literal "auto" (permitted by the type, though nothing writes it
    // today) would map to `false` and read as "Off". Binding directly to the string means
    // "auto" always means Auto, whatever wrote it.
    it("Markers: a stored literal \"auto\" displays as Auto, not Off", () => {
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], markers: "auto" }} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Markers")).getByText("Auto")).toBeInTheDocument();
    });
  });
});

// TWO metrics throughout, because Metabase hides the whole control once one stackable series is
// left (`stackable.stack_type`'s getHidden) and these are about what it READS, not about when it
// appears. The appearing is covered in viz/__tests__/cartesian.controls.test.tsx, through the
// descriptor that feeds it the series the chart actually draws.
describe("CartesianDisplaySection — Stacking", () => {
  // Metabase offers one control with three options. Two switches made the reader discover the
  // third state by turning the first one on, and could store `normalized` while `stacked` was off.
  it("shows one control, and a fresh area reads Stack", () => {
    render(<CartesianDisplaySection kind="area" config={{ yMetrics: ["rev", "cnt"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Stacking")).toHaveTextContent("Stack");
    expect(screen.queryByLabelText("100% stacked")).toBeNull();
  });

  it("a fresh bar reads Don't stack", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev", "cnt"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Stacking")).toHaveTextContent(/Don.t stack/);
  });

  // Stack totals only means something on a plain stack — a 100% stack's total is always 100.
  it("offers Stack totals on a stack but not on a 100% stack", () => {
    const { rerender } = render(
      <CartesianDisplaySection kind="area" config={{ yMetrics: ["rev", "cnt"] }} onChange={() => {}} />,
    );
    expect(screen.getByLabelText("Stack totals")).toBeInTheDocument();
    rerender(
      <CartesianDisplaySection kind="area" config={{ yMetrics: ["rev", "cnt"], stacked: true, normalized: true }} onChange={() => {}} />,
    );
    expect(screen.queryByLabelText("Stack totals")).toBeNull();
  });
});

describe("CartesianDisplaySection — axis titles and the split", () => {
  // With the text box as the only control, hiding a title meant emptying it, and the wording was
  // gone for good. Metabase pairs each title with its own switch.
  it("keeps Show label separate from the label text", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabel: "Month" }} onChange={onChange} />,
    );
    expect(screen.getByLabelText("Show X label")).toBeChecked();
    expect(screen.getByLabelText("X label")).toHaveValue("Month");
    fireEvent.click(screen.getByLabelText("Show X label"));
    expect(onChange).toHaveBeenCalledWith({ showXLabel: false });
    // Hidden: the box goes away, and the stored text is untouched so turning it back on restores it.
    rerender(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabel: "Month", showXLabel: false }} onChange={onChange} />);
    expect(screen.queryByLabelText("X label")).toBeNull();
    fireEvent.click(screen.getByLabelText("Show X label"));
    expect(onChange).toHaveBeenLastCalledWith({ showXLabel: undefined });
  });

  // Nothing to separate with one metric.
  it("offers the split only once there are two metrics to separate", () => {
    const { rerender } = render(
      <CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />,
    );
    expect(screen.queryByLabelText("Split y-axis when necessary")).toBeNull();
    rerender(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev", "cnt"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Split y-axis when necessary")).toBeChecked();
  });
});

describe("CartesianDisplaySection — the per-metric axis select", () => {
  const cfg = { yMetrics: ["rev", "cnt"] };

  // It used to read "Left" for every metric while the automatic split was drawing one of them on
  // the right — the panel asserting something the chart visibly contradicted.
  it("rests on Auto while nothing is assigned by hand", () => {
    render(<CartesianDisplaySection kind="bar" config={cfg} onChange={() => {}} />);
    expect(screen.getByLabelText("axis rev")).toHaveTextContent("Auto");
    expect(screen.getByLabelText("axis cnt")).toHaveTextContent("Auto");
  });

  it("shows the real sides once one is assigned", () => {
    render(<CartesianDisplaySection kind="bar" config={{ ...cfg, rightAxisMetrics: ["cnt"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("axis rev")).toHaveTextContent("Left");
    expect(screen.getByLabelText("axis cnt")).toHaveTextContent("Right");
  });
});

/**
 * THE X-AXIS-SCALE SELECT, and the one place this PR could have made egret LESS like Metabase.
 *
 * The select was hidden on scatter while hiding it was inert — a scatter always drew a category
 * axis, which is what the hidden control would have said. `resolveXScale` ended that: a scatter over
 * a numeric dimension is now drawn on a value axis nobody chose, and with the control hidden the
 * automatic answer would also have been the only answer, underneath a rotation select greyed out
 * with an explanation and no lever to act on. Metabase computes the same default and shows it
 * (`ScatterPlot.tsx` spreads `GRAPH_AXIS_SETTINGS`, carrying `graph.x_axis.scale`).
 */
describe("CartesianDisplaySection — the x-axis scale select", () => {
  it("offers the scale on a scatter with a dimension on x, where it was hidden before", () => {
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={() => {}} xColumnType="double" />);
    expect(screen.getByLabelText("X axis scale")).toBeInTheDocument();
  });

  /** The trigger reads the RESOLVED scale. Reading `config.xScale` would print "Ordinal" underneath
   *  a chart visibly plotting values across, which is worse than not offering the control at all. */
  it("shows Linear on a numeric scatter that stored no scale at all", () => {
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={() => {}} xColumnType="double" />);
    expect(screen.getByLabelText("X axis scale")).toHaveTextContent("Linear");
  });

  it("shows Ordinal on a varchar scatter, which is what that one is drawn on", () => {
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "region" }} onChange={() => {}} xColumnType="varchar" />);
    expect(screen.getByLabelText("X axis scale")).toHaveTextContent("Ordinal");
  });

  /** The lever itself: choosing Ordinal on an auto-linear scatter must STORE it, because a stored
   *  scale is the only thing `resolveXScale` lets outrank the automatic one. */
  it("stores an explicit Ordinal, which is how the automatic value axis is overridden", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={onChange} xColumnType="double" />);
    await user.click(screen.getByLabelText("X axis scale"));
    await user.click(await screen.findByRole("option", { name: "Ordinal" }));
    expect(onChange).toHaveBeenCalledWith({ xScale: "ordinal" });
  });

  /**
   * A METRIC-VS-METRIC scatter keeps the exclusion, and for a different reason than the one that
   * has just been retired: it leaves through `scatterXY`, which builds its own two value axes and
   * never reads `xScale`. Offering it there would store a setting nothing consults.
   */
  it("still hides the scale on a metric-vs-metric scatter, whose axes ignore it", () => {
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xMetric: "cnt" }} onChange={() => {}} xColumnType="double" />);
    expect(screen.queryByLabelText("X axis scale")).toBeNull();
  });

  /** Unchanged: a row chart and a flipped bar draw their categories down the left, where
   *  `usesXScale`'s `!isHorizontal` guard means an x-scale would be stored and ignored. */
  it.each([
    ["row" as const, {}],
    ["bar" as const, { horizontal: true }],
  ])("still hides the scale on a %s chart", (kind, extra) => {
    render(<CartesianDisplaySection kind={kind} config={{ yMetrics: ["rev"], xAxis: "price", ...extra }} onChange={() => {}} xColumnType="double" />);
    expect(screen.queryByLabelText("X axis scale")).toBeNull();
  });

  /**
   * A SCALE THE COLUMN CANNOT CARRY IS OFFERED GREYED, NOT LIVE.
   *
   * Measured live on `MB · Bar`, whose x is a text column: all three items were enabled and picking
   * Timeseries drew an entirely empty plot area. Metabase, driven side by side on the same card,
   * disables the select down to Ordinal alone there.
   *
   * The panel is the second half of the fix — `resolveXScale` refuses the same combination, which
   * is what reaches a card that already has the bad value stored (`xScaleFitsColumn.test.ts`). This
   * half is about not offering it in the first place.
   *
   * Asserted on `aria-disabled`, which is what a Radix `SelectItem` sets and the only signal a
   * screen reader gets — `disabled` as an attribute is not there to find.
   */
  const scaleItems = async (user: ReturnType<typeof userEvent.setup>) => {
    await user.click(screen.getByLabelText("X axis scale"));
    const state = async (name: string) =>
      (await screen.findByRole("option", { name })).getAttribute("aria-disabled");
    return { linear: await state("Linear"), timeseries: await state("Timeseries"), ordinal: await state("Ordinal") };
  };

  it("greys Linear and Timeseries out on a text x column, leaving Ordinal", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxis: "category" }} onChange={() => {}} xColumnType="varchar" />);
    expect(await scaleItems(user)).toEqual({ linear: "true", timeseries: "true", ordinal: null });
  });

  /** A number is not a date: `Date.parse("15.69")` is NaN, so Timeseries empties a numeric chart
   *  just as it empties a text one. Metabase offers Linear / Power / Log / Ordinal there and no
   *  Timeseries. */
  it("greys only Timeseries out on a numeric x column", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={() => {}} xColumnType="double" />);
    expect(await scaleItems(user)).toEqual({ linear: null, timeseries: "true", ordinal: null });
  });

  it("greys only Linear out on a temporal x column", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxis: "d" }} onChange={() => {}} xColumnType="timestamp" />);
    expect(await scaleItems(user)).toEqual({ linear: "true", timeseries: null, ordinal: null });
  });

  /** Before a query has run there is no result column to judge against, and greying an item out on
   *  a guess would refuse a setting that may be exactly right. Nothing is disabled. */
  it("greys nothing out while the column type is unknown", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxis: "category" }} onChange={() => {}} />);
    expect(await scaleItems(user)).toEqual({ linear: null, timeseries: null, ordinal: null });
  });

  /** A card saved with the bad value shows what it is DRAWN as, not what it stored — the trigger
   *  reads the resolved scale, and the resolution now refuses this pair. The stored key is
   *  untouched, so re-pointing the chart at a date column brings Timeseries straight back. */
  it("reads Ordinal on a text column that stored Timeseries, which is what it now draws", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxis: "category", xScale: "timeseries" }} onChange={() => {}} xColumnType="varchar" />);
    expect(screen.getByLabelText("X axis scale")).toHaveTextContent("Ordinal");
  });
});

describe("CartesianDisplaySection — a combo can stack", () => {
  // Metabase hides the per-series display on `stackable.stack_type`, which only means anything
  // because a combo can reach that state there. egret excluded combo from `canStack`, so a combo
  // could not stack at all and the guard that hides the display select was unreachable.
  // Both series named as bars, because a FRESH combo draws its first metric as a line and Metabase
  // hides Stacking wherever one stackable series is left — see the getHidden suite in
  // viz/__tests__/cartesian.controls.test.tsx, which owns that rule.
  it("offers Stacking on a combo", () => {
    render(
      <CartesianDisplaySection
        kind="combo"
        config={{ yMetrics: ["rev", "cnt"], seriesTypes: { rev: "bar", cnt: "bar" } }}
        onChange={() => {}}
      />,
    );
    expect(screen.getByLabelText("Stacking")).toHaveTextContent(/Don.t stack/);
  });

  it("still offers none where stacking makes no sense", () => {
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Stacking")).toBeNull();
  });
});

/**
 * The category axis' four-way control, which replaced a switch. It is ONE select over TWO stored
 * fields, so every assertion here is on the emitted patch rather than on the DOM: the failure mode
 * this guards is a patch that sets half the pair and leaves the other half saying something else.
 * `toStrictEqual` on purpose — `toEqual` ignores `undefined` properties, so it cannot tell a patch
 * that CLEARS the other key from one that silently leaves it alone.
 */
describe("CartesianDisplaySection — the category-axis lines/ticks/rotation select", () => {
  const open = async (user: ReturnType<typeof userEvent.setup>, label = "X show lines and tick marks") =>
    user.click(screen.getByLabelText(label));

  it("choosing Rotate 45° stores the rotation and clears the hide", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxisTicks: false }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Rotate 45°" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 45, xAxisTicks: undefined, xLabelCompact: undefined });
  });

  it("choosing Rotate 90° stores 90, not 45", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Rotate 90°" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 90, xAxisTicks: undefined, xLabelCompact: undefined });
  });

  it("choosing Hide turns the axis off and drops the rotation with it", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 45 }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Hide" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xAxisTicks: false, xLabelRotation: undefined, xLabelCompact: undefined });
  });

  // The path back to "let the builder decide": Show must clear the rotation, or the measured fit
  // stays suppressed forever by a setting the user believes they have turned off.
  it("choosing Show clears the rotation", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90 }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Show" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xAxisTicks: undefined, xLabelRotation: undefined, xLabelCompact: undefined });
  });

  it("displays what is stored: a rotation, and Hide winning over one", () => {
    const { rerender } = render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90 }} onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Rotate 90°");
    rerender(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90, xAxisTicks: false }} onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Hide");
  });

  // A row chart's categories run down the LEFT, so this control names the vertical axis — and the
  // rotation is real there: ECharts applies `axisLabel.rotate` to a y-axis too.
  it("names the category axis Y on a row chart and still offers the rotation", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="row" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await open(user, "Y show lines and tick marks");
    const opt = await screen.findByRole("option", { name: "Rotate 45°" });
    expect(opt).not.toHaveAttribute("aria-disabled", "true");
    await user.click(opt);
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 45, xAxisTicks: undefined, xLabelCompact: undefined });
  });

  /**
   * A linear or timeseries x is a VALUE axis: its labels are ticks ECharts generates, and
   * `scaleAxis` carries no `rotate`, so a rotation chosen there would store and do nothing. Offered
   * disabled rather than removed, so a chart that stored 45° before its scale changed still shows
   * what it stored instead of an empty control. PR 03 did not change that — it changed which charts
   * ARRIVE on that axis, which is the numeric-scatter case further down.
   */
  it.each(["linear", "timeseries"] as const)("disables the rotations on a %s x-axis", async (xScale) => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], xScale }} onChange={() => {}} />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).toHaveAttribute("aria-disabled", "true");
    expect(await screen.findByRole("option", { name: "Rotate 90°" })).toHaveAttribute("aria-disabled", "true");
    // Hiding the axis line still works on a value axis, so those two stay live.
    expect(await screen.findByRole("option", { name: "Hide" })).not.toHaveAttribute("aria-disabled", "true");
  });

  /**
   * …but a HORIZONTAL chart is not a value-x chart, however its `xScale` reads. The x-scale select
   * hides itself once a chart is horizontal, so a stored `linear` there is a leftover from before
   * it was flipped — and the builder ignores it (`usesXScale` is `!isHorizontal && …`), drawing the
   * ordinal categories down the left where the rotation does apply. Disabling on it would take a
   * working setting away on the strength of a value nothing reads.
   */
  it("keeps them enabled on a horizontal chart carrying a leftover linear scale", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], horizontal: true, xScale: "linear" }} onChange={() => {}} />);
    await open(user, "Y show lines and tick marks");
    expect(await screen.findByRole("option", { name: "Rotate 90°" })).not.toHaveAttribute("aria-disabled", "true");
  });

  it("leaves them enabled on an ordinal x-axis", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).not.toHaveAttribute("aria-disabled", "true");
  });

  // metric-vs-metric: this scatter leaves through `scatterXY` before a category axis is ever built.
  it("disables them on a metric-vs-metric scatter", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xMetric: "cnt" }} onChange={() => {}} />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 90°" })).toHaveAttribute("aria-disabled", "true");
  });

  /** …and the converse, which is the half of that arm doing the work: a scatter with a DIMENSION on
   *  x falls through to `baseOption` like any other chart, so testing `kind === "scatter"` alone
   *  would grey out a live setting on every one of them — and every other assertion in this file
   *  passes under that mutation. Which axis it lands on now depends on the COLUMN, which is the
   *  pair of cases below; with no type to go on it stays enabled, the answer the two after that
   *  pin. */
  it("leaves them enabled on a scatter with a varchar x-axis", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "region" }} onChange={onChange} xColumnType="varchar" />);
    await open(user);
    const opt = await screen.findByRole("option", { name: "Rotate 45°" });
    expect(opt).not.toHaveAttribute("aria-disabled", "true");
    await user.click(opt);
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 45, xAxisTicks: undefined, xLabelCompact: undefined });
  });

  /**
   * THE CASE PR 03 CREATED. A scatter over a numeric dimension is drawn on a value axis with
   * nothing at all stored on `xScale` (`resolveXScale`), so a rotation chosen here would store and
   * do nothing — the same dead control the two rows above are greyed out for, reached without the
   * user having chosen anything. Reading `config.xScale` alone, as this predicate did before, leaves
   * it live.
   */
  it("disables them on a scatter over a numeric dimension, which is now a value axis", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={() => {}} xColumnType="double" />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/Rotation and Compact apply to a category axis/)).toBeInTheDocument();
  });

  /** A numeric column under any other kind is still a category axis — the builder's rule is scatter
   *  only, and this predicate has to carry the same restriction or it greys out a working control
   *  on every bar chart grouped by a number. */
  it("leaves them enabled on a bar chart over a numeric dimension", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={() => {}} xColumnType="double" />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).not.toHaveAttribute("aria-disabled", "true");
  });

  /** Before a query has run there is no result column to read a type off. Staying enabled is the
   *  honest answer to "unknown" — the alternative greys the control out on charts that will honour
   *  it, on the strength of a type nobody has looked up yet. */
  it("leaves them enabled on a scatter whose column type is not known yet", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xAxis: "price" }} onChange={() => {}} />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).not.toHaveAttribute("aria-disabled", "true");
  });

  /**
   * WHERE THE REASON LIVES, and why it is asserted at all. It first sat on the disabled items as a
   * native `title`, which no class of user could reach: `data-[disabled]:pointer-events-none` stops
   * a mouse hovering them, Radix skips disabled items in its roving focus so a keyboard never lands
   * on one, and a screen reader is handed `aria-disabled` and nothing else. So the reason hangs off
   * the trigger — focusable, pointer-events intact — and is wired with `aria-describedby`, which is
   * what puts it in the accessibility tree rather than merely on screen.
   */
  it("explains the greying-out on the trigger, where it can actually be read", async () => {
    const user = userEvent.setup();
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], xScale: "timeseries" }} onChange={() => {}} />);
    const hint = screen.getByText(/Rotation and Compact apply to a category axis/);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveAttribute("aria-describedby", hint.id);
    // …and NOT on the items, where it would look like an explanation and reach nobody.
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).not.toHaveAttribute("title");
  });

  it("says nothing where every choice is live", () => {
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.queryByText(/Rotation and Compact apply to a category axis/)).toBeNull();
    expect(screen.getByLabelText("X show lines and tick marks")).not.toHaveAttribute("aria-describedby");
  });

  it("still shows a rotation stored before the scale changed", () => {
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], xScale: "linear", xLabelRotation: 45 }} onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Rotate 45°");
  });

  /**
   * METABASE'S FIFTH CHOICE, WHICH THIS PANEL DEFERRED FOR THREE PRs on a reason that turned out to
   * be scoped to one of its four x-scales. The comment on the control carries the correction; these
   * pin that it is offered on the ordinal x every bucketed temporal chart actually draws, and greyed
   * on the `scaleAxis` case that really is out of reach.
   */
  it("offers Compact on an ordinal x and greys it on a value x, exactly as the rotations are", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Compact" })).not.toHaveAttribute("aria-disabled", "true");
    await user.keyboard("{Escape}");
    rerender(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], xScale: "timeseries" }} onChange={() => {}} />);
    await open(user);
    expect(await screen.findByRole("option", { name: "Compact" })).toHaveAttribute("aria-disabled", "true");
  });

  it("stores Compact and clears the rotation it replaces", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90 }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Compact" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({
      xLabelCompact: true, xAxisTicks: undefined, xLabelRotation: undefined,
    });
  });
});

/**
 * METABASE'S `graph.y_axis.unpin_from_zero`, and what this panel is responsible for is WHERE IT
 * APPEARS — the arithmetic is the builder's and is pinned in `echarts/__tests__/unpinFromZero.test.ts`.
 *
 * Every arm below is a state a user reaches in ONE CLICK of a neighbouring control, which is why
 * the row's condition calls the builder's `unpinFromZeroApplies` rather than restating it: a second
 * copy of that rule here is how a control comes to be offered on a chart whose builder ignores it.
 * Each assertion pairs a hidden case with a shown one, so none of them can pass against a row that
 * is simply never rendered.
 */
describe("CartesianDisplaySection — Unpin from zero appears where Metabase offers it", () => {
  const unpin = () => screen.queryByLabelText("Unpin from zero");

  it("is offered on a line and refused on a bar, an area and a row", () => {
    const { rerender } = render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(unpin()).toBeInTheDocument();
    for (const kind of ["bar", "area", "row"] as const) {
      rerender(<CartesianDisplaySection kind={kind} config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
      expect(unpin()).toBeNull();
    }
  });

  it("judges a combo by its SERIES, so changing one series' type moves the row", () => {
    // A fresh combo is line-then-bar (`seriesDisplayFor`'s default), so it has a bar in it and is
    // refused — which is what the live MB · Combo does. Naming both series Line brings it back.
    const { rerender } = render(<CartesianDisplaySection kind="combo" config={{ yMetrics: ["rev", "orders"] }} onChange={() => {}} />);
    expect(unpin()).toBeNull();
    rerender(<CartesianDisplaySection kind="combo"
      config={{ yMetrics: ["rev", "orders"], seriesTypes: { rev: "line", orders: "line" } }} onChange={() => {}} />);
    expect(unpin()).toBeInTheDocument();
  });

  it("disappears when the value range stops being automatic", () => {
    const { rerender } = render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(unpin()).toBeInTheDocument();
    // `{ yMin: 0 }` is exactly the patch `ValueRangeRows` emits when its switch is turned off, so
    // this is the state one click away and not a fixture invented for the test.
    rerender(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], yMin: 0 }} onChange={() => {}} />);
    expect(unpin()).toBeNull();
  });

  it("disappears once the chart stacks", () => {
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev", "orders"], stacked: true }} onChange={() => {}} />);
    expect(unpin()).toBeNull();
  });

  /**
   * THE SCATTER IS OFFERED IT, AND THESE REPLACE A TEST ASSERTING THE OPPOSITE.
   *
   * The first draft excluded the scatter, reasoning that `scatterXY` reads none of this panel's Y
   * value-axis keys. That was true until this PR made `scatterXY` read `unpinFromZero` — a scatter
   * is the one display Metabase unpins by default, so the builder had to honour it. The exception
   * went into the BUILDER and not the PANEL, which left every saved scatter silently unpinned with
   * no control to pin it back: a behaviour change a user cannot undo, which is the "implemented and
   * unreachable" family rather than a missing nicety.
   *
   * The three below are one story — it is SHOWN, it shows the RESOLVED state rather than the stored
   * one, and turning it off PERSISTS — and each can break without the other two noticing.
   */
  it("is offered on a scatter, and checked by default as Metabase's is", () => {
    render(<CartesianDisplaySection kind="scatter" config={{ yMetrics: ["rev"], xMetric: "price" }} onChange={() => {}} />);
    expect(unpin()).toBeInTheDocument();
    // The RESOLVED value. Reading the stored key would draw this OFF under a chart that is visibly
    // unpinned — a control that lies, not a cosmetic slip.
    expect(unpin()).toBeChecked();
  });

  it("draws a non-scatter unchecked by default, so `checked` cannot be hard-coded", () => {
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(unpin()).not.toBeChecked();
  });

  it("persists a scatter turned OFF as an explicit false, because its default is on", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const cfg = { yMetrics: ["rev"], xMetric: "price" };
    const { rerender } = render(<CartesianDisplaySection kind="scatter" config={cfg} onChange={onChange} />);
    await user.click(screen.getByLabelText("Unpin from zero"));
    // NOT `undefined`: on a scatter that means "take the default", which is ON — so the switch would
    // spring straight back and the pin would be unsavable. That is the same failure `ValueRangeRows`
    // records for its own derived switch, reached from the other side.
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ unpinFromZero: false });
    // …and turning it back on CLEARS the key rather than storing `true`, since true is the default
    // here. Round-tripping to the same absence a fresh scatter has is the point.
    rerender(<CartesianDisplaySection kind="scatter" config={{ ...cfg, unpinFromZero: false }} onChange={onChange} />);
    expect(unpin()).not.toBeChecked();
    await user.click(screen.getByLabelText("Unpin from zero"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ unpinFromZero: undefined });
  });

  it("stores true and clears the key rather than storing false, where the default is off", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Unpin from zero"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ unpinFromZero: true });
    rerender(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], unpinFromZero: true }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Unpin from zero"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ unpinFromZero: undefined });
  });
});

describe("CartesianDisplaySection — axis names follow the chart's orientation", () => {
  // `xLabel` is the CATEGORY axis and `yLabel` the value axis, whichever way the bars run. On a row
  // chart the category axis is the vertical one, so naming these after the config keys printed them
  // backwards from the screen: "Y min" governed the horizontal extent. Metabase relabels the same
  // settings by the axis they appear on.
  // A stored bound throughout, because the pair is gated behind the automatic range now — and the
  // SWITCH is named for the axis too, which is the row that would silently keep saying "y" if the
  // shared control had hard-coded its label.
  it("names the value-axis controls X on a row chart", () => {
    render(<CartesianDisplaySection kind="row" config={{ yMetrics: ["rev"], yMin: 0 }} onChange={() => {}} />);
    expect(screen.getByLabelText("Auto x-axis range")).toBeInTheDocument();
    expect(screen.getByLabelText("X min")).toBeInTheDocument();
    expect(screen.getByLabelText("X max")).toBeInTheDocument();
    expect(screen.getByLabelText("Number of tick marks")).toBeInTheDocument();
    expect(screen.getByLabelText("X show lines and tick marks")).toBeInTheDocument();
    // …and the category axis, which is vertical here, reads Y.
    expect(screen.getByLabelText("Y show lines and tick marks")).toBeInTheDocument();
    expect(screen.getByLabelText("Show Y label")).toBeInTheDocument();
  });

  it("does the same for a bar turned horizontal", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], horizontal: true, yMin: 0 }} onChange={() => {}} />);
    expect(screen.getByLabelText("Auto x-axis range")).toBeInTheDocument();
    expect(screen.getByLabelText("X min")).toBeInTheDocument();
    expect(screen.getByLabelText("Y show lines and tick marks")).toBeInTheDocument();
  });

  it("leaves an upright chart alone", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yMin: 0 }} onChange={() => {}} />);
    expect(screen.getByLabelText("Auto y-axis range")).toBeInTheDocument();
    expect(screen.getByLabelText("Y min")).toBeInTheDocument();
    expect(screen.getByLabelText("X show lines and tick marks")).toBeInTheDocument();
    expect(screen.getByLabelText("Show X label")).toBeInTheDocument();
  });
});

/**
 * The value axis' Scale row — Metabase's `graph.y_axis.scale`, as a select rather than the
 * "Log scale" switch cartesian used to carry. The waterfall/box-plot tab already renders one
 * (`ChartAxesSection`); two panels offering the same stored setting through two different
 * controls is how the same key comes to read differently depending on which chart you opened.
 *
 * Asserted through to the BUILT OPTION, not stopped at the emitted patch: a select that emits
 * `{ logScale: true }` into a builder that ignored it would still be a Scale row that changes
 * nothing on screen, and the patch-only assertion could not tell the difference.
 */
describe("AxisRangeOptions — the value axis' Scale select", () => {
  const cols = [{ name: "x", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["a", 1], ["b", 10]];

  const axisTypeAfterChoosing = async (from: CartesianConfig, option: string) => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<AxisRangeOptions kind="bar" config={from} onChange={onChange} />);
    await user.click(screen.getByLabelText("Y scale"));
    await user.click(await screen.findByRole("option", { name: option }));
    const patch = onChange.mock.calls.at(-1)![0] as Partial<CartesianConfig>;
    const built = buildCartesianOption(cols, rows, {
      kind: "bar", xAxis: "x", ...from, ...patch,
    }) as { yAxis: { type: string } };
    return built.yAxis.type;
  };

  it("Log builds a log value axis", async () => {
    expect(await axisTypeAfterChoosing({ yMetrics: ["rev"] }, "Log")).toBe("log");
  });

  it("Linear builds a plain value axis", async () => {
    expect(await axisTypeAfterChoosing({ yMetrics: ["rev"], logScale: true }, "Linear")).toBe("value");
  });

  /* Metabase's third option, Power, is deliberately absent — see the comment on the control. */
  it("offers exactly Linear and Log", async () => {
    const user = userEvent.setup();
    render(<AxisRangeOptions kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    await user.click(screen.getByLabelText("Y scale"));
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual(["Linear", "Log"]);
  });

  /** A horizontal chart's measure runs across, and every other row on this control already
   *  renames itself accordingly. */
  it("names itself after the axis the reader sees on a row chart", () => {
    render(<AxisRangeOptions kind="row" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("X scale")).toBeInTheDocument();
    expect(screen.queryByLabelText("Y scale")).toBeNull();
  });
});

/*
 * THE PANEL-SIDE COLOUR RULE IS ASSERTED IN `viz/__tests__/cartesian.controls.test.tsx`, not here.
 *
 * A block of tests stood at this spot comparing a locally-rebuilt swatch against the chart's own
 * series. It caught the defect it was written for, but its `panelSwatch` helper RESTATED the line
 * the app actually runs (`SeriesListControl`), so it could not see that line being wrong — proved
 * by mutating that line and watching the whole chart suite stay green. Those cases now render the
 * control the descriptor declares and read the swatch out of the DOM, which is coverage of the
 * decision rather than of a copy of it.
 */

/**
 * ONE AXES VOCABULARY, NOT TWO.
 *
 * MEASURED live across six charts. The waterfall and the box plot render their Axes tab through
 * `ChartAxesSection` and match Metabase: an "Auto y-axis range" switch that gates Y min / Y max, and
 * "Show lines and tick marks" as a SELECT. Bar, line, area, combo, row and scatter render through
 * `CartesianDisplaySection` and got a different panel over THE SAME STORED KEYS — no auto-range
 * switch at all (a bare Y min / Y max pair, always visible), and lines/ticks as a switch.
 *
 * Which panel a reader saw depended only on which chart they had opened.
 *
 * The gate costs no stored key: `autoRange` is derived, `config.yMin == null && config.yMax == null`,
 * so there is no state in which the switch and the boxes can disagree and nothing new is persisted.
 */
describe("the cartesian Axes tab speaks the same vocabulary as the waterfall's", () => {
  const bar = (config: CartesianConfig) =>
    render(<CartesianDisplaySection kind="bar" config={config} onChange={() => {}} />);

  it("hides the bounds behind Auto y-axis range, as the waterfall's tab does", () => {
    bar({ yMetrics: ["rev"] });
    expect(screen.getByLabelText("Auto y-axis range")).toBeChecked();
    expect(screen.queryByLabelText("Y min")).toBeNull();
    expect(screen.queryByLabelText("Y max")).toBeNull();
  });

  /** Turning it off has to REVEAL the pair, and seed a bound — an "off" that showed two empty boxes
   *  would put the switch and the values it is derived from into immediate disagreement. */
  it("reveals the bounds when the automatic range is turned off", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Auto y-axis range"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ yMin: 0 });
  });

  /** …and a chart that already stores a bound arrives with the switch off and the boxes open, since
   *  the switch is derived from the bounds rather than stored beside them. */
  it("shows a stored bound with the switch already off", () => {
    bar({ yMetrics: ["rev"], yMax: 50 });
    expect(screen.getByLabelText("Auto y-axis range")).not.toBeChecked();
    expect(screen.getByLabelText("Y min")).toBeInTheDocument();
    expect(screen.getByLabelText("Y max")).toBeInTheDocument();
  });

  /** Turning it back ON has to clear BOTH bounds, or the switch reads "auto" over a chart still
   *  pinned to a stored maximum. */
  it("clears both bounds when the automatic range is turned back on", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yMax: 50 }} onChange={onChange} />);
    // `toStrictEqual` on the emitted patch, not `toHaveBeenCalledWith`: the latter runs the same
    // loose equality `toEqual` does, which IGNORES `undefined` properties — so a patch clearing only
    // `yMin` and leaving a stored `yMax` behind satisfied it, and a mutation doing exactly that
    // survived the whole suite. The switch would then have read "auto" over a chart still pinned to
    // its maximum. This file already makes the same argument on the x-ticks patches.
    fireEvent.click(screen.getByLabelText("Auto y-axis range"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ yMin: undefined, yMax: undefined });
  });

  /**
   * The value axis' lines and ticks as a SELECT, which is what the other tab offers and what
   * Metabase's `graph.y_axis.axis_enabled` is. Asserted as a combobox carrying both choices rather
   * than merely "not a switch": a select that rendered one option would satisfy the negative.
   */
  it("offers the value axis' lines and ticks as a select, not a switch", async () => {
    const user = userEvent.setup();
    bar({ yMetrics: ["rev"] });
    const trigger = screen.getByLabelText("Y show lines and tick marks");
    expect(trigger).toHaveAttribute("role", "combobox");
    await user.click(trigger);
    expect(await screen.findByRole("option", { name: "Hide" })).toBeInTheDocument();
    expect(await screen.findByRole("option", { name: "Show" })).toBeInTheDocument();
  });

  it("stores the hide and clears it again through that select", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Y show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Hide" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ yAxisTicks: false });

    cleanup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yAxisTicks: false }} onChange={onChange} />);
    await user.click(screen.getByLabelText("Y show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Show" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ yAxisTicks: undefined });
  });

  /**
   * THE WORDING ITSELF, pinned by rendering both panels and comparing what a reader can address.
   * This is the assertion that would have caught the divergence in the first place: every other
   * test in either file passes happily while the two tabs name the same setting differently.
   *
   * `ChartAxesSection` is rendered with its own default id prefix, so nothing here depends on the
   * cartesian ids.
   */
  it("names the shared rows exactly as the waterfall/box-plot tab names them", () => {
    const shared = ["Auto y-axis range", "Y min", "Y max", "Y show lines and tick marks", "Number of tick marks"];
    render(<ChartAxesSection config={{ yMin: 0 }} onChange={() => {}} />);
    for (const label of shared) expect(screen.getByLabelText(label), `${label} on the waterfall tab`).toBeInTheDocument();
    cleanup();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yMin: 0 }} onChange={() => {}} />);
    for (const label of shared) expect(screen.getByLabelText(label), `${label} on the cartesian tab`).toBeInTheDocument();
  });

  /**
   * …AND THE HORIZONTAL RELABELLING SURVIVES IT. A row chart's measure runs ACROSS, so the value
   * axis is the X one and every row in this block is named for it — the relabelling the cartesian
   * panel has done all along and which `ChartAxesSection`, whose two charts are always vertical,
   * has no need of. Sharing the controls must not flatten it back to a hard-coded "Y".
   */
  it("still names the value axis after the axis the reader sees on a row chart", () => {
    render(<CartesianDisplaySection kind="row" config={{ yMetrics: ["rev"], yMin: 0 }} onChange={() => {}} />);
    expect(screen.getByLabelText("Auto x-axis range")).toBeInTheDocument();
    expect(screen.getByLabelText("X min")).toBeInTheDocument();
    expect(screen.getByLabelText("X show lines and tick marks")).toBeInTheDocument();
  });
});
