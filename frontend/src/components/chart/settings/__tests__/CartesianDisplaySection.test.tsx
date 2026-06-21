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
});
