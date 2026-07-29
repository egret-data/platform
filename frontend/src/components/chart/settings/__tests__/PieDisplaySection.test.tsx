import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PieDisplaySection } from "../PieDisplaySection";

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

  it("Decimal places patches decimals as a whole number", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ percentVisibility: "inside" }} onChange={onChange} slices={[]} />);
    fireEvent.change(screen.getByLabelText("Decimal places"), { target: { value: "0" } });
    expect(onChange).toHaveBeenCalledWith({ decimals: 0 });
  });
});
