import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { PieDisplaySection } from "../PieDisplaySection";

describe("PieDisplaySection", () => {
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
