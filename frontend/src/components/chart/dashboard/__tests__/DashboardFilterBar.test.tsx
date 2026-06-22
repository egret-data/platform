import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardFilterBar } from "../DashboardFilterBar";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "contains" },
  { id: "p2", name: "Amount", slug: "amount", type: "number", operator: "between" },
  { id: "p3", name: "Status", slug: "status", type: "category", operator: "is" },
];

describe("DashboardFilterBar", () => {
  it("renders a labelled widget per parameter and reports text changes", () => {
    const onChange = vi.fn();
    render(<DashboardFilterBar parameters={params} values={{}} onChange={onChange} optionsBySlug={{ status: ["new", "paid"] }} />);
    expect(screen.getByText("Region")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Region"), { target: { value: "EU" } });
    expect(onChange).toHaveBeenCalledWith("region", "EU");
  });
  it("number between renders two inputs", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.getByLabelText("Amount min")).toBeInTheDocument();
    expect(screen.getByLabelText("Amount max")).toBeInTheDocument();
  });
  it("category renders a multi-select listbox with the provided options", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{ status: ["new", "paid"] }} />);
    expect(screen.getByLabelText("Status")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "new" })).toBeInTheDocument();
  });
});
