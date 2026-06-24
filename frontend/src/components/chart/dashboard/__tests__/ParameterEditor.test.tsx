import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ParameterEditor } from "../ParameterEditor";
import type { DashboardParameter } from "@/api/dashboards";

const param: DashboardParameter = { id: "p1", name: "Region", slug: "region", type: "text", operator: "is" };

describe("ParameterEditor", () => {
  it("edits the name", () => {
    const onChange = vi.fn();
    render(<ParameterEditor param={param} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText(/parameter name/i), { target: { value: "Area" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ name: "Area" }));
  });
  it("changing type resets to that type's first operator", () => {
    const onChange = vi.fn();
    render(<ParameterEditor param={param} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText(/parameter type/i), { target: { value: "number" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ type: "number", operator: "=" }));
  });
  it("deletes", () => {
    const onDelete = vi.fn();
    render(<ParameterEditor param={param} onChange={vi.fn()} onDelete={onDelete} />);
    fireEvent.click(screen.getByRole("button", { name: /delete parameter/i }));
    expect(onDelete).toHaveBeenCalled();
  });
  it("temporal_unit type sets default month and hides the operator", () => {
    let p: DashboardParameter = { id: "p1", name: "P", slug: "p", type: "text", operator: "is" };
    const onChange = (n: DashboardParameter) => { p = n; };
    const { rerender } = render(<ParameterEditor param={p} onChange={onChange} onDelete={() => {}} />);
    fireEvent.change(screen.getByLabelText("parameter type"), { target: { value: "temporal_unit" } });
    expect(p.type).toBe("temporal_unit");
    expect(p.default).toBe("month");
    rerender(<ParameterEditor param={p} onChange={onChange} onDelete={() => {}} />);
    expect(screen.queryByLabelText("parameter operator")).toBeNull();
    expect(screen.getByLabelText("default unit")).toBeTruthy();
  });
});
