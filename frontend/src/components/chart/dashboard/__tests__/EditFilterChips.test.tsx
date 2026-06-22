import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EditFilterChips } from "../EditFilterChips";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "a", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "b", name: "Priority", slug: "priority", type: "category", operator: "is" },
];

describe("EditFilterChips", () => {
  it("renders a chip per param, marks the active one, and selects on click", () => {
    const onSelect = vi.fn();
    render(<EditFilterChips parameters={params} activeId="a" onSelect={onSelect} />);
    expect(screen.getByRole("button", { name: "Region" }).getAttribute("data-active")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Priority" }));
    expect(onSelect).toHaveBeenCalledWith("b");
  });
});
