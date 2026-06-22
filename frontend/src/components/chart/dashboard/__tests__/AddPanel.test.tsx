import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { AddPanel } from "../AddPanel";
import type { Chart } from "@/api/charts";

const charts = [{ id: "1", name: "Revenue bar" }, { id: "2", name: "Orders line" }] as unknown as Chart[];

describe("AddPanel", () => {
  it("filters the chart list by search and adds on click", () => {
    const onAddChart = vi.fn();
    render(<AddPanel charts={charts} onAddChart={onAddChart} onAddVirtual={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search charts"), { target: { value: "order" } });
    expect(screen.queryByRole("button", { name: "Revenue bar" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Orders line" }));
    expect(onAddChart).toHaveBeenCalledWith(charts[1]);
  });
  it("adds virtual cards", () => {
    const onAddVirtual = vi.fn();
    render(<AddPanel charts={charts} onAddChart={vi.fn()} onAddVirtual={onAddVirtual} />);
    fireEvent.click(screen.getByRole("button", { name: "Add Text" }));
    expect(onAddVirtual).toHaveBeenCalledWith("text");
  });
});
