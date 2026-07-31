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
    // Unset means shown, so a chart saved before these existed keeps drawing both.
    expect(screen.getByLabelText("X axis lines and ticks")).toBeChecked();
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
