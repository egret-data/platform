import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { AddPanel } from "../AddPanel";
import type { Chart } from "@/api/charts";

const charts = [{ id: "1", name: "Revenue bar", viz_type: "bar" }, { id: "2", name: "Orders line", viz_type: "line" }] as unknown as Chart[];

describe("AddPanel", () => {
  it("renders a viz-type icon on each chart row", () => {
    render(<AddPanel charts={charts} onAddChart={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Revenue bar" }).querySelector("svg")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Orders line" }).querySelector("svg")).toBeTruthy();
  });
  it("filters the chart list by search and adds on click", () => {
    const onAddChart = vi.fn();
    render(<AddPanel charts={charts} onAddChart={onAddChart} />);
    fireEvent.change(screen.getByLabelText("search charts"), { target: { value: "order" } });
    expect(screen.queryByRole("button", { name: "Revenue bar" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Orders line" }));
    expect(onAddChart).toHaveBeenCalledWith(charts[1]);
  });
});
