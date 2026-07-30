import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { FieldZone } from "../FieldZone";

describe("FieldZone display names", () => {
  it("shows a field's verbose name instead of its machine name", () => {
    // Datasets have carried `verbose_name` for a long time and offer a UI to set it, but the chart
    // studio read only the machine name — so naming a column "Total Sales" changed nothing here.
    render(<FieldZone label="Values" selected={["total_revenue"]} options={["total_revenue"]}
      labels={{ total_revenue: "Total Sales" }} onChange={vi.fn()} />);
    expect(screen.getByText("Total Sales")).toBeInTheDocument();
    expect(screen.queryByText("total_revenue")).not.toBeInTheDocument();
  });

  it("falls back to the machine name when there is no verbose name", () => {
    render(<FieldZone label="Rows" selected={["region"]} options={["region"]} onChange={vi.fn()} />);
    expect(screen.getByText("region")).toBeInTheDocument();
  });

  it("treats a blank verbose name as absent", () => {
    render(<FieldZone label="Rows" selected={["region"]} options={["region"]}
      labels={{ region: "   " }} onChange={vi.fn()} />);
    expect(screen.getByText("region")).toBeInTheDocument();
  });

  it("names the field in the drag and remove controls, not just 'drag'", () => {
    // Four identical "drag" buttons tell a screen-reader user nothing about which row they are on.
    render(<FieldZone label="Rows" selected={["region"]} options={["region"]}
      labels={{ region: "Region" }} onChange={vi.fn()} />);
    expect(screen.getByRole("button", { name: "reorder Region" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "remove Region" })).toBeInTheDocument();
  });
});

describe("FieldZone labels reach the cartesian zones too", () => {
  it("a metric's verbose name shows in a Y-axis zone", () => {
    render(<FieldZone label="Y-axis" selected={["total_revenue"]} options={["total_revenue"]}
      labels={{ total_revenue: "Tổng doanh thu" }} onChange={vi.fn()} />);
    expect(screen.getByText("Tổng doanh thu")).toBeInTheDocument();
  });
});
