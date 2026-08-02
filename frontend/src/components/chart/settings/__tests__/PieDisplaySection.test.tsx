import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PieDisplaySection } from "../PieDisplaySection";
import { buildPieOption } from "@/components/chart/echarts/options/pie";

describe("PieDisplaySection", () => {
  // Legend and Show labels both default from buildPieOption at render time (a pie's colours
  // and slices are unreadable without them), so they are a three-way Auto/On/Off select
  // rather than a Switch — `undefined` is a real, visible state, not "off". A two-state
  // Switch bound to `!!config.showLegend` would show OFF while the chart draws ON, the same
  // "switch that lies" bug already fixed for chart 01's legend/compact-numbers and chart 02's
  // connect-gaps. Each interaction starts from a value that differs from the target, since a
  // Radix Select does not fire onValueChange when the already-selected item is re-clicked.
  describe("Legend (Auto/On/Off)", () => {
    it("choosing On emits true", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Legend"));
      await user.click(await screen.findByRole("option", { name: "On" }));
      expect(onChange).toHaveBeenCalledWith({ showLegend: true });
    });

    it("choosing Off emits false", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Legend"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ showLegend: false });
    });

    it("choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ showLegend: true }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Legend"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ showLegend: undefined });
    });

    it("shows Auto selected when the config leaves it unset", () => {
      render(<PieDisplaySection config={{}} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Legend")).getByText("Auto")).toBeInTheDocument();
    });
  });

  describe("Show labels (Auto/On/Off)", () => {
    it("choosing On emits true", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show labels"));
      await user.click(await screen.findByRole("option", { name: "On" }));
      expect(onChange).toHaveBeenCalledWith({ showLabels: true });
    });

    it("choosing Off emits false", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show labels"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ showLabels: false });
    });

    it("choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ showLabels: true }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show labels"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ showLabels: undefined });
    });

    it("shows Auto selected when the config leaves it unset", () => {
      render(<PieDisplaySection config={{}} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Show labels")).getByText("Auto")).toBeInTheDocument();
    });
  });

  // `showTotal` now defaults ON for a multi-ring pie (buildPieOption resolves it at render
  // time), so a two-state Switch bound to `!!config.showTotal` would read OFF while the chart
  // draws the total — the same "switch that lies" this section already fixed for Legend and
  // Show labels. It also has to be REACHABLE on a multi-ring pie, which need not have `donut`
  // set at all: the hole is there either way, and hiding the control behind `donut` would leave
  // a newly-on-by-default total with no way to turn it off.
  describe("Show total (Auto/On/Off)", () => {
    it("is offered for a multi-ring pie even with donut unset", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Show total")).toBeInTheDocument();
    });

    it("is still offered for a single-ring donut", () => {
      render(<PieDisplaySection config={{ donut: true, dimensions: ["a"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Show total")).toBeInTheDocument();
    });

    it("is not offered for a single-ring pie with no hole to write in", () => {
      render(<PieDisplaySection config={{ dimensions: ["a"] }} onChange={() => {}} />);
      expect(screen.queryByLabelText("Show total")).not.toBeInTheDocument();
    });

    it("shows Auto when nothing is stored, rather than claiming Off", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Show total")).getByText("Auto")).toBeInTheDocument();
    });

    it("choosing Off emits false", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show total"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ showTotal: false });
    });

    it("choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ dimensions: ["a", "b"], showTotal: true }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show total"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ showTotal: undefined });
    });
  });

  it("per-slice colour swatch patches sliceColors by (raw) name", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{}} onChange={onChange} slices={["A", "B"]} />);
    fireEvent.change(screen.getByLabelText("color B"), { target: { value: "#ff0000" } });
    expect(onChange).toHaveBeenCalledWith({ sliceColors: { B: "#ff0000" } });
  });

  it("per-slice rename patches sliceNames and clears (deletes the key) when emptied", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ sliceNames: { A: "Old" } }} onChange={onChange} slices={["A"]} />);
    fireEvent.change(screen.getByLabelText("label A"), { target: { value: "Alpha" } });
    expect(onChange).toHaveBeenCalledWith({ sliceNames: { A: "Alpha" } });
    fireEvent.change(screen.getByLabelText("label A"), { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith({ sliceNames: {} });
  });

  it("toggling a slice's switch off emits hiddenSlices containing it", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{}} onChange={onChange} slices={["A", "B"]} />);
    fireEvent.click(screen.getByLabelText("Show B"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSlices: ["B"] });
  });

  it("toggling a hidden slice's switch back on removes it from hiddenSlices", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ hiddenSlices: ["B"] }} onChange={onChange} slices={["A", "B"]} />);
    fireEvent.click(screen.getByLabelText("Show B"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSlices: [] });
  });

  it("ranks rows by the stored sliceOrder, keeping unranked slices after it in their natural place", () => {
    render(<PieDisplaySection config={{ sliceOrder: ["C", "A"] }} onChange={() => {}} slices={["A", "B", "C"]} />);
    // Filter to the per-slice switches: the section has other Switches too (Legend, Donut,
    // Show labels…) whose aria-labels don't follow the "Show <slice>" shape.
    const rows = screen.getAllByRole("switch")
      .map((el) => el.getAttribute("aria-label"))
      .filter((l): l is string => !!l && ["Show A", "Show B", "Show C"].includes(l));
    expect(rows).toEqual(["Show C", "Show A", "Show B"]);
  });

  it("moving the last slice up swaps it with its neighbor", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{}} onChange={onChange} slices={["A", "B", "C"]} />);
    fireEvent.click(screen.getByLabelText("Move C up"));
    expect(onChange).toHaveBeenCalledWith({ sliceOrder: ["A", "C", "B"] });
  });

  it("reordering visible slices merges into the stored order rather than replacing it", () => {
    const onChange = vi.fn();
    // "B" is named in the stored order but isn't present in this run's slices.
    render(<PieDisplaySection config={{ sliceOrder: ["A", "B", "C"] }} onChange={onChange} slices={["A", "C"]} />);
    fireEvent.click(screen.getByLabelText("Move C up"));
    const [{ sliceOrder }] = onChange.mock.calls[0];
    expect(sliceOrder).toContain("B");
    expect(sliceOrder.indexOf("C")).toBeLessThan(sliceOrder.indexOf("A"));
  });

  it("disables the up-arrow on the first row and the down-arrow on the last", () => {
    render(<PieDisplaySection config={{}} onChange={() => {}} slices={["A", "B"]} />);
    expect(screen.getByLabelText("Move A up")).toBeDisabled();
    expect(screen.getByLabelText("Move B down")).toBeDisabled();
    expect(screen.getByLabelText("Move A down")).not.toBeDisabled();
    expect(screen.getByLabelText("Move B up")).not.toBeDisabled();
  });

  it("the slice rows do not render when there are no slices", () => {
    render(<PieDisplaySection config={{}} onChange={() => {}} slices={[]} />);
    expect(screen.queryByText("Slices")).not.toBeInTheDocument();
  });

  it("Decimal places is hidden until a percent visibility is chosen", () => {
    render(<PieDisplaySection config={{}} onChange={() => {}} slices={[]} />);
    expect(screen.queryByLabelText("Decimal places")).not.toBeInTheDocument();
  });

  // `legend` completes Metabase's own `pie.percent_visibility` domain — it is the value its
  // sample sunburst is configured with, and the one egret was missing.
  describe("Percentages", () => {
    const openPercentages = async (user: ReturnType<typeof userEvent.setup>) => {
      await user.click(screen.getByLabelText("Percentages"));
    };

    it("offers In legend alongside the values that were already there", async () => {
      const user = userEvent.setup();
      render(<PieDisplaySection config={{}} onChange={() => {}} slices={[]} />);
      await openPercentages(user);
      const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
      expect(options).toEqual(["Auto", "Off", "In legend", "In chart", "Both"]);
    });

    it("choosing In legend emits the legend value", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} slices={[]} />);
      await openPercentages(user);
      await user.click(await screen.findByRole("option", { name: "In legend" }));
      expect(onChange).toHaveBeenCalledWith({ percentVisibility: "legend" });
    });

    it("choosing Auto clears the stored value", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ percentVisibility: "both" }} onChange={onChange} slices={[]} />);
      await openPercentages(user);
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ percentVisibility: undefined });
    });

    // The control has to show what the chart will draw. With nothing stored, a multi-ring pie
    // draws its percentages in the legend, so a select without Auto would read "Off" while
    // the legend carried numbers.
    it("shows Auto when nothing is stored", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} slices={[]} />);
      expect(within(screen.getByLabelText("Percentages")).getByText("Auto")).toBeInTheDocument();
    });

    it("offers Decimal places under Auto on a multi-ring pie, since Auto draws percentages there", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} slices={[]} />);
      expect(screen.getByLabelText("Decimal places")).toBeInTheDocument();
    });

    it("keeps Decimal places hidden under Auto on a single-ring pie, which draws none", () => {
      render(<PieDisplaySection config={{ dimensions: ["a"] }} onChange={() => {}} slices={[]} />);
      expect(screen.queryByLabelText("Decimal places")).not.toBeInTheDocument();
    });

    // Whatever the chart does with an unrecognised stored value, the control must say the same
    // thing — and the chart draws nothing.
    it("reports an unknown stored value as Off, matching what the chart draws", () => {
      render(
        <PieDisplaySection
          config={{ dimensions: ["a", "b"], percentVisibility: "everywhere" as never }}
          onChange={() => {}} slices={[]} />,
      );
      expect(within(screen.getByLabelText("Percentages")).getByText("Off")).toBeInTheDocument();
    });
  });

  it("Decimal places patches decimals as a whole number", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ percentVisibility: "inside" }} onChange={onChange} slices={[]} />);
    fireEvent.change(screen.getByLabelText("Decimal places"), { target: { value: "0" } });
    expect(onChange).toHaveBeenCalledWith({ decimals: 0 });
  });

  // The chart derives its own precision from the shares it draws once nothing is stored, so a
  // number in this box with nothing behind it would be a control claiming a value the chart isn't
  // using — the fault this panel already fixed for the legend, the labels and the centre total.
  it("Decimal places shows Auto, not a number, when nothing is stored", () => {
    render(<PieDisplaySection config={{ percentVisibility: "inside" }} onChange={() => {}} slices={[]} />);
    const input = screen.getByLabelText("Decimal places") as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("Auto");
  });

  it("Decimal places shows a stored precision, and clearing it goes back to Auto", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ percentVisibility: "inside", decimals: 3 }} onChange={onChange} slices={[]} />);
    expect((screen.getByLabelText("Decimal places") as HTMLInputElement).value).toBe("3");
    fireEvent.change(screen.getByLabelText("Decimal places"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ decimals: undefined });
  });
});

/**
 * THE SWATCH IS THE COLOUR THE PIE DRAWS.
 *
 * The default was `SERIES_PALETTE[i % length]` — a by-POSITION rule the renderer stopped following
 * when ring 0 moved to `colorsForKeys`. So these tests BUILD THE PIE and read ring 0's colours back,
 * rather than comparing the panel to `colorsForKeys` (which would prove wiring, not agreement).
 */
describe("PieDisplaySection — the slice swatch and the slice the chart draws", () => {
  const cols = [{ name: "category", type: "varchar" }, { name: "m", type: "double" }];
  /**
   * Value-desc, which is ring 0's own emission order, and deliberately NOT alphabetical: on the hash
   * path `colorsForKeys` assigns in sorted key order and so returns its Record in sorted order, and
   * a pre-sorted fixture makes a by-name lookup and a by-position one hand back the same sequence.
   * "Gadget" and "Widget" also hash onto one slot, so this walks the collision path too.
   */
  const rows: unknown[][] = [["Widget", 40], ["Doohickey", 30], ["Gizmo", 20], ["Gadget", 10]];
  /** What `PieDisplayControl` (viz/simple.tsx) hands the panel: the distinct raw values of the first
   *  dimension, in row order. */
  const SLICES = ["Widget", "Doohickey", "Gizmo", "Gadget"];

  /** Ring 0's colours, keyed by slice name. */
  const pieColors = (cfg: Record<string, unknown> = {}): Record<string, string> => {
    const opt = buildPieOption(cols, rows, { dimensions: ["category"], metric: "m", ...cfg }) as unknown as {
      series: { data: { name: string; itemStyle: { color: string } }[] }[];
    };
    return Object.fromEntries(opt.series[0].data.map((d) => [d.name, d.itemStyle.color.toLowerCase()]));
  };
  /** A native `<input type="color">` normalises its value to lower-case (HTML spec), which is also
   *  what the panel writes back — so both sides are compared in that casing. */
  const swatch = (name: string) => (screen.getByLabelText(`color ${name}`) as HTMLInputElement).value.toLowerCase();

  it("shows each slice the colour the pie draws it in, not the palette slot of its row", () => {
    const chart = pieColors();
    expect(Object.keys(chart)).toEqual(SLICES); // the emission order this pins against
    render(<PieDisplaySection config={{ dimensions: ["category"] }} onChange={() => {}} slices={SLICES} />);
    for (const name of SLICES) expect(swatch(name)).toBe(chart[name]);
  });

  it("keeps a slice's swatch when the rows are reordered", () => {
    const { rerender } = render(<PieDisplaySection config={{ dimensions: ["category"] }} onChange={() => {}} slices={SLICES} />);
    const before = Object.fromEntries(SLICES.map((n) => [n, swatch(n)]));
    rerender(<PieDisplaySection config={{ dimensions: ["category"] }} onChange={() => {}} slices={[...SLICES].reverse()} />);
    for (const name of SLICES) expect(swatch(name)).toBe(before[name]);
  });

  /** `sliceColors` reaches ring 0 as `colorsForKeys`' `existing` map, where an entry CONSUMES its
   *  colour — so a pin moves whichever slice would have hashed onto it, in the chart and here. */
  it("folds a pinned colour in, so the slice it displaced moves here too", () => {
    const sliceColors = { Gadget: "#98d9d9" }; // SERIES_PALETTE[6], lower-cased as the picker gives it back
    const chart = pieColors({ sliceColors });
    render(<PieDisplaySection config={{ dimensions: ["category"], sliceColors }} onChange={() => {}} slices={SLICES} />);
    for (const name of SLICES) expect(swatch(name)).toBe(chart[name]);
    expect(new Set(SLICES.map(swatch)).size).toBe(4); // nobody was handed the pinned ink twice
  });

  /**
   * A hidden slice's rows are dropped before the ring is built (buildPieRings), so it is not a key
   * and spends no colour: the slices that remain must show what the chart now draws them in.
   *
   * GADGET IS THE ONE TO HIDE, not Widget. The two collide on one palette slot, and "Gadget" sorts
   * first and takes it while "Widget" walks along — so hiding Gadget frees that slot and MOVES
   * Widget, which is the only way this fixture can tell a re-keyed panel from one that quietly kept
   * every name. Hiding Widget instead leaves the other three exactly where they were, and this test
   * passed with the filter deleted.
   */
  it("re-keys on the slices that survive a hide, as the chart does", () => {
    const chart = pieColors({ hiddenSlices: ["Gadget"] });
    expect(Object.keys(chart)).not.toContain("Gadget");
    expect(chart["Widget"]).not.toBe(pieColors()["Widget"]); // the premise: hiding Gadget moved it
    render(<PieDisplaySection config={{ dimensions: ["category"], hiddenSlices: ["Gadget"] }} onChange={() => {}} slices={SLICES} />);
    for (const name of Object.keys(chart)) expect(swatch(name)).toBe(chart[name]);
    // …and the hidden row still shows something: what it will be given when it is turned back on.
    expect(swatch("Gadget")).toBe(pieColors()["Gadget"]);
  });
});
