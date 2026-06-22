import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("FilterPill", () => {
  it("shows the param name and value summary, and a clear button when set", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p()} value={["EU", "US"]} options={["EU", "US", "APAC"]} onChange={onChange} />);
    expect(screen.getByRole("button", { name: /^Region/ }).textContent).toContain("2 selections");
    fireEvent.click(screen.getByRole("button", { name: "clear Region" }));
    expect(onChange).toHaveBeenCalledWith([]);
  });

  it("category popover: search filters options and checking one calls onChange with an array", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p()} value={[]} options={["EU", "US", "APAC"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Region/ }));
    fireEvent.change(screen.getByLabelText("search Region"), { target: { value: "ap" } });
    expect(screen.queryByLabelText("option EU")).toBeNull();
    fireEvent.click(screen.getByLabelText("option APAC"));
    expect(onChange).toHaveBeenCalledWith(["APAC"]);
  });

  it("id popover: single-select calls onChange with a scalar", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "id", name: "User", slug: "user", operator: "is" })} value="" options={["1", "2"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^User/ }));
    fireEvent.click(screen.getByLabelText("option 2"));
    expect(onChange).toHaveBeenCalledWith("2");
  });

  it("text popover: typing calls onChange with the string", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "text", name: "Name", slug: "name", operator: "contains" })} value="" options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Name/ }));
    fireEvent.change(screen.getByLabelText("Name value"), { target: { value: "ann" } });
    expect(onChange).toHaveBeenCalledWith("ann");
  });

  it("number between: two inputs call onChange with [min, max]", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "number", name: "Price", slug: "price", operator: "between" })} value={[null, null]} options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Price/ }));
    fireEvent.change(screen.getByLabelText("Price min"), { target: { value: "3" } });
    expect(onChange).toHaveBeenCalledWith([3, null]);
  });

  it("date relative: choosing an option calls onChange with the code", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "date", name: "When", slug: "when", operator: "relative" })} value="" options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^When/ }));
    fireEvent.click(screen.getByLabelText("Last 30 days"));
    expect(onChange).toHaveBeenCalledWith("last30");
  });
});
