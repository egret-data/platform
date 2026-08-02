import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CartesianDisplaySection } from "../CartesianDisplaySection";

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

  it("offers Metabase's axis line/tick toggles and the value-axis tick count", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    // Unset means shown, so a chart saved before these existed keeps drawing both. The category
    // axis is a four-way select (it carries the rotation too); the value axis is still a switch.
    expect(screen.getByLabelText("X axis lines and ticks")).toHaveTextContent("Show");
    expect(screen.getByLabelText("Y axis lines and ticks")).toBeChecked();
    fireEvent.click(screen.getByLabelText("Y axis lines and ticks"));
    expect(onChange).toHaveBeenCalledWith({ yAxisTicks: false });
    fireEvent.change(screen.getByLabelText("Y tick marks"), { target: { value: "5" } });
    expect(onChange).toHaveBeenLastCalledWith({ yTickCount: 5 });
  });

  it("clearing the tick count returns the axis to auto", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yTickCount: 5 }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Y tick marks"), { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith({ yTickCount: undefined });
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

  it("Y min/max inputs patch yMin/yMax (undefined when cleared)", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yMax: 50 }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Y min"), { target: { value: "0" } });
    expect(onChange).toHaveBeenCalledWith({ yMin: 0 });
    fireEvent.change(screen.getByLabelText("Y max"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ yMax: undefined });
  });

  // Task 7: line shape, missing-value and marker controls, gated on isLineish (line/area).
  // Per-series style/size live in their own control (SeriesLineStyleSection, tested
  // separately) so they can hide behind "More options"; shape and markers stay
  // chart-wide-only everywhere (see task doc for the deliberate gap).
  describe("line shape / missing values / markers (isLineish only)", () => {
    it("all three render for line", () => {
      render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Line shape")).toBeInTheDocument();
      expect(screen.getByLabelText("Missing values")).toBeInTheDocument();
      expect(screen.getByLabelText("Markers")).toBeInTheDocument();
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

describe("CartesianDisplaySection — Stacking", () => {
  // Metabase offers one control with three options. Two switches made the reader discover the
  // third state by turning the first one on, and could store `normalized` while `stacked` was off.
  it("shows one control, and a fresh area reads Stack", () => {
    render(<CartesianDisplaySection kind="area" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Stacking")).toHaveTextContent("Stack");
    expect(screen.queryByLabelText("100% stacked")).toBeNull();
  });

  it("a fresh bar reads Don't stack", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Stacking")).toHaveTextContent(/Don.t stack/);
  });

  // Stack totals only means something on a plain stack — a 100% stack's total is always 100.
  it("offers Stack totals on a stack but not on a 100% stack", () => {
    const { rerender } = render(
      <CartesianDisplaySection kind="area" config={{ yMetrics: ["rev"] }} onChange={() => {}} />,
    );
    expect(screen.getByLabelText("Stack totals")).toBeInTheDocument();
    rerender(
      <CartesianDisplaySection kind="area" config={{ yMetrics: ["rev"], stacked: true, normalized: true }} onChange={() => {}} />,
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
});

describe("CartesianDisplaySection — a combo can stack", () => {
  // Metabase hides the per-series display on `stackable.stack_type`, which only means anything
  // because a combo can reach that state there. egret excluded combo from `canStack`, so a combo
  // could not stack at all and the guard that hides the display select was unreachable.
  it("offers Stacking on a combo", () => {
    render(<CartesianDisplaySection kind="combo" config={{ yMetrics: ["rev", "cnt"] }} onChange={() => {}} />);
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
  const open = async (user: ReturnType<typeof userEvent.setup>, label = "X axis lines and ticks") =>
    user.click(screen.getByLabelText(label));

  it("choosing Rotate 45° stores the rotation and clears the hide", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xAxisTicks: false }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Rotate 45°" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 45, xAxisTicks: undefined });
  });

  it("choosing Rotate 90° stores 90, not 45", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Rotate 90°" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 90, xAxisTicks: undefined });
  });

  it("choosing Hide turns the axis off and drops the rotation with it", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 45 }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Hide" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xAxisTicks: false, xLabelRotation: undefined });
  });

  // The path back to "let the builder decide": Show must clear the rotation, or the measured fit
  // stays suppressed forever by a setting the user believes they have turned off.
  it("choosing Show clears the rotation", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90 }} onChange={onChange} />);
    await open(user);
    await user.click(await screen.findByRole("option", { name: "Show" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xAxisTicks: undefined, xLabelRotation: undefined });
  });

  it("displays what is stored: a rotation, and Hide winning over one", () => {
    const { rerender } = render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90 }} onChange={() => {}} />);
    expect(screen.getByLabelText("X axis lines and ticks")).toHaveTextContent("Rotate 90°");
    rerender(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], xLabelRotation: 90, xAxisTicks: false }} onChange={() => {}} />);
    expect(screen.getByLabelText("X axis lines and ticks")).toHaveTextContent("Hide");
  });

  // A row chart's categories run down the LEFT, so this control names the vertical axis — and the
  // rotation is real there: ECharts applies `axisLabel.rotate` to a y-axis too.
  it("names the category axis Y on a row chart and still offers the rotation", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="row" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    await open(user, "Y axis lines and ticks");
    const opt = await screen.findByRole("option", { name: "Rotate 45°" });
    expect(opt).not.toHaveAttribute("aria-disabled", "true");
    await user.click(opt);
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 45, xAxisTicks: undefined });
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
    await open(user, "Y axis lines and ticks");
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
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 45, xAxisTicks: undefined });
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
    expect(screen.getByText(/Rotation applies to a category axis/)).toBeInTheDocument();
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
    const hint = screen.getByText(/Rotation applies to a category axis/);
    expect(screen.getByLabelText("X axis lines and ticks")).toHaveAttribute("aria-describedby", hint.id);
    // …and NOT on the items, where it would look like an explanation and reach nobody.
    await open(user);
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).not.toHaveAttribute("title");
  });

  it("says nothing where every choice is live", () => {
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.queryByText(/Rotation applies to a category axis/)).toBeNull();
    expect(screen.getByLabelText("X axis lines and ticks")).not.toHaveAttribute("aria-describedby");
  });

  it("still shows a rotation stored before the scale changed", () => {
    render(<CartesianDisplaySection kind="line" config={{ yMetrics: ["rev"], xScale: "linear", xLabelRotation: 45 }} onChange={() => {}} />);
    expect(screen.getByLabelText("X axis lines and ticks")).toHaveTextContent("Rotate 45°");
  });
});

describe("CartesianDisplaySection — axis names follow the chart's orientation", () => {
  // `xLabel` is the CATEGORY axis and `yLabel` the value axis, whichever way the bars run. On a row
  // chart the category axis is the vertical one, so naming these after the config keys printed them
  // backwards from the screen: "Y min" governed the horizontal extent. Metabase relabels the same
  // settings by the axis they appear on.
  it("names the value-axis controls X on a row chart", () => {
    render(<CartesianDisplaySection kind="row" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("X min")).toBeInTheDocument();
    expect(screen.getByLabelText("X max")).toBeInTheDocument();
    expect(screen.getByLabelText("X tick marks")).toBeInTheDocument();
    expect(screen.getByLabelText("X axis lines and ticks")).toBeInTheDocument();
    // …and the category axis, which is vertical here, reads Y.
    expect(screen.getByLabelText("Y axis lines and ticks")).toBeInTheDocument();
    expect(screen.getByLabelText("Show Y label")).toBeInTheDocument();
  });

  it("does the same for a bar turned horizontal", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], horizontal: true }} onChange={() => {}} />);
    expect(screen.getByLabelText("X min")).toBeInTheDocument();
    expect(screen.getByLabelText("Y axis lines and ticks")).toBeInTheDocument();
  });

  it("leaves an upright chart alone", () => {
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={() => {}} />);
    expect(screen.getByLabelText("Y min")).toBeInTheDocument();
    expect(screen.getByLabelText("X axis lines and ticks")).toBeInTheDocument();
    expect(screen.getByLabelText("Show X label")).toBeInTheDocument();
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
