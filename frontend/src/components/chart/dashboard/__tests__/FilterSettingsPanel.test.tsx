import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p: DashboardParameter = { id: "a", name: "Region", slug: "region", type: "category", operator: "is" };

describe("FilterSettingsPanel", () => {
  it("edits name and required, and deletes", () => {
    const onChange = vi.fn(); const onDelete = vi.fn();
    render(<FilterSettingsPanel param={p} onChange={onChange} onDelete={onDelete} />);
    fireEvent.change(screen.getByLabelText("parameter name"), { target: { value: "Reg" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ name: "Reg" }));
    fireEvent.click(screen.getByLabelText("parameter required"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ required: true }));
    fireEvent.click(screen.getByRole("button", { name: "delete filter" }));
    expect(onDelete).toHaveBeenCalled();
  });
  it("changing type resets the operator to the first for that type", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("parameter type"), { target: { value: "number" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ type: "number", operator: "=" }));
  });
});
