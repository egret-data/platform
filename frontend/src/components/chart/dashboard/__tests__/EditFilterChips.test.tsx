import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EditFilterChips, FILTER_DRAG_ACTIVATION_PX, reorderParameters } from "../EditFilterChips";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "a", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "b", name: "Priority", slug: "priority", type: "category", operator: "is" },
  { id: "c", name: "Owner", slug: "owner", type: "category", operator: "is" },
];

describe("EditFilterChips", () => {
  it("renders a chip per param, marks the active one, and selects on click", () => {
    const onSelect = vi.fn();
    render(<EditFilterChips parameters={params} activeId="a" onSelect={onSelect} onReorder={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Region" }).getAttribute("data-active")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Priority" }));
    expect(onSelect).toHaveBeenCalledWith("b");
  });

  it("names each drag handle after its filter", () => {
    // Three identical "drag" buttons tell a screen-reader user nothing about which chip they hold.
    render(<EditFilterChips parameters={params} activeId={null} onSelect={vi.fn()} onReorder={vi.fn()} />);
    expect(screen.getByRole("button", { name: "reorder Region" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "reorder Owner" })).toBeTruthy();
  });

  it("uses Metabase's 15px drag activation distance", () => {
    // A chip is also a click target (it opens the settings panel). Metabase's own list sets
    // `useDndSensors({ distance: 15 })` (ParametersList.tsx:51) for exactly that reason; below
    // this, a click that wanders a pixel starts a drag instead of opening the panel.
    expect(FILTER_DRAG_ACTIVATION_PX).toBe(15);
  });
});

describe("reorderParameters", () => {
  it("moves the dragged parameter to the position it was dropped on", () => {
    expect(reorderParameters(params, "c", "a").map((p) => p.id)).toEqual(["c", "a", "b"]);
    expect(reorderParameters(params, "a", "c").map((p) => p.id)).toEqual(["b", "c", "a"]);
  });

  it("returns the same array reference for a no-op, so the caller can skip the write", () => {
    expect(reorderParameters(params, "b", "b")).toBe(params);
    expect(reorderParameters(params, "b", "zzz")).toBe(params);
  });
});
