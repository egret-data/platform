import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

  it("temporal_unit renders a unit dropdown and selects a unit", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<FilterPill
      param={{ id: "p", name: "Date grouping", slug: "dg", type: "temporal_unit", operator: "is", default: "month" }}
      value="month" options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Date grouping"));
    await user.click(screen.getByRole("combobox", { name: "Date grouping value" }));
    // The two groups survive the move off <optgroup>.
    expect(screen.getByText("Group by")).toBeInTheDocument();
    expect(screen.getByText("Extract")).toBeInTheDocument();
    await user.click(await screen.findByRole("option", { name: "Quarter" }));
    expect(onChange).toHaveBeenCalledWith("quarter");
  });

  it("single-value category emits the RAW option, not an array, and renders no checkboxes", () => {
    // Keying on the raw value matters: the code stores what it emits here, and a test that
    // asserted the label "EU" would pass against an implementation storing ["EU"].
    const onChange = vi.fn();
    render(<FilterPill param={p({ isMultiSelect: false })} value="" options={["EU", "US"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Region/ }));
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    fireEvent.click(screen.getByLabelText("option US"));
    expect(onChange).toHaveBeenCalledWith("US");
  });

  it("multi-value id emits an array once the author opts in", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "id", name: "User", slug: "user", isMultiSelect: true })}
      value={[]} options={["1", "2"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^User/ }));
    fireEvent.click(screen.getByLabelText("option 2"));
    expect(onChange).toHaveBeenCalledWith(["2"]);
  });

  it("clearing a single-value category yields \"\", not []", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ isMultiSelect: false })} value="EU" options={["EU"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "clear Region" }));
    expect(onChange).toHaveBeenCalledWith("");
  });
});
