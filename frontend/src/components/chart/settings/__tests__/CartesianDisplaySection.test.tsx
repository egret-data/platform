import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CartesianDisplaySection } from "../CartesianDisplaySection";

describe("CartesianDisplaySection", () => {
  it("toggling Legend patches showLegend", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Legend"));
    expect(onChange).toHaveBeenCalledWith({ showLegend: true });
  });
  it("Horizontal shows for bar, hides for line", () => {
    const { rerender } = render(<CartesianDisplaySection kind="bar" config={{ yMetrics: [] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Horizontal")).toBeInTheDocument();
    rerender(<CartesianDisplaySection kind="line" config={{ yMetrics: [] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Horizontal")).not.toBeInTheDocument();
  });
  it("per-series colour swatch patches seriesColors by name", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev", "cnt"] }} onChange={onChange} seriesNames={["rev", "cnt"]} />);
    fireEvent.change(screen.getByLabelText("color cnt"), { target: { value: "#ff0000" } });
    expect(onChange).toHaveBeenCalledWith({ seriesColors: { cnt: "#ff0000" } });
  });
  it("per-series rename patches seriesLabels and clears when emptied", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], seriesLabels: { rev: "Old" } }} onChange={onChange} seriesNames={["rev"]} />);
    fireEvent.change(screen.getByLabelText("label rev"), { target: { value: "Revenue" } });
    expect(onChange).toHaveBeenCalledWith({ seriesLabels: { rev: "Revenue" } });
    fireEvent.change(screen.getByLabelText("label rev"), { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith({ seriesLabels: {} });
  });
  it("Y min/max inputs patch yMin/yMax (undefined when cleared)", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"], yMax: 50 }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Y min"), { target: { value: "0" } });
    expect(onChange).toHaveBeenCalledWith({ yMin: 0 });
    fireEvent.change(screen.getByLabelText("Y max"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ yMax: undefined });
  });
});
