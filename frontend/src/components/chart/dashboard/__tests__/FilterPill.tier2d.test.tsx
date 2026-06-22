import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "is", ...over });

const openPill = (param: DashboardParameter, value: unknown, opts: string[], onChange = vi.fn()) => {
  render(<FilterPill param={param} value={value} options={opts} onChange={onChange} />);
  fireEvent.click(screen.getByRole("button", { name: param.name }));
  return onChange;
};

describe("FilterPill tier2d", () => {
  it("boolean renders a 3-state Any/True/False select and encodes the choice", () => {
    const onChange = openPill(p({ type: "boolean", operator: "is" }), "", []);
    const sel = screen.getByLabelText("F value") as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(["", "true", "false"]);
    fireEvent.change(sel, { target: { value: "true" } });
    expect(onChange).toHaveBeenCalledWith("true");
  });

  it("widget=input renders a text input for a category and writes a scalar string", () => {
    const onChange = openPill(p({ type: "category", operator: "is", widget: "input" }), "", ["a", "b"]);
    const input = screen.getByLabelText("F value") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "hello" } });
    expect(onChange).toHaveBeenCalledWith("hello");
  });

  it("location renders a multi-checkbox list", () => {
    const onChange = openPill(p({ type: "location", operator: "is" }), [], ["US", "CA"]);
    fireEvent.click(screen.getByLabelText("option US").querySelector("button,[role=checkbox]") ?? screen.getByLabelText("option US"));
    expect(onChange).toHaveBeenCalledWith(["US"]);
  });

  it("month-year renders an <input type=month>", () => {
    const onChange = openPill(p({ type: "date", operator: "month-year" }), "", []);
    const input = screen.getByLabelText("F value") as HTMLInputElement;
    expect(input.type).toBe("month");
    fireEvent.change(input, { target: { value: "2024-03" } });
    expect(onChange).toHaveBeenCalledWith("2024-03");
  });

  it("quarter-year renders a year input + a quarter select and encodes 'YYYY-Q'", () => {
    const onChange = openPill(p({ type: "date", operator: "quarter-year" }), "", []);
    fireEvent.change(screen.getByLabelText("F year"), { target: { value: "2024" } });
    expect(onChange).toHaveBeenCalledWith("2024-1");
    fireEvent.change(screen.getByLabelText("F quarter"), { target: { value: "3" } });
    expect(onChange).toHaveBeenCalledWith("2024-3");
  });
});
