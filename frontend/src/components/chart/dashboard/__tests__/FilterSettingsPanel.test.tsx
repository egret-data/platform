import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("FilterSettingsPanel tier2d", () => {
  it("type select offers boolean and location", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    const sel = screen.getByLabelText("parameter type") as HTMLSelectElement;
    const opts = Array.from(sel.options).map((o) => o.value);
    expect(opts).toContain("boolean");
    expect(opts).toContain("location");
  });

  it("widget radio writes param.widget", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("widget search"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ widget: "search" }));
  });

  it("default control writes param.default", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("default value"), { target: { value: "West" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ default: "West" }));
  });

  it("linked-filters checkbox writes filteringParameters with other param ids", () => {
    const onChange = vi.fn();
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({})} allParameters={[p({}), other]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("link City"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ filteringParameters: ["2"] }));
  });

  it("does not offer the param itself as a linked filter", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("link Region")).toBeNull();
  });
});
