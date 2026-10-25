import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "text", operator: "is", ...over });

const openOptions = async (param: DashboardParameter) => {
  const user = userEvent.setup();
  render(<FilterSettingsPanel param={param} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
  await user.click(screen.getByRole("combobox", { name: "parameter operator" }));
  return (await screen.findAllByRole("option")).map((o) => o.textContent);
};

// KI-280: the Operator select named the operators by their internal keys (`is-not`, `all-options`).
describe("FilterSettingsPanel operator labels (KI-280)", () => {
  it("offers a text filter's operators by label", async () => {
    expect(await openOptions(p())).toEqual(
      ["Is", "Is not", "Contains", "Does not contain", "Starts with", "Ends with"]);
  });

  it("offers a number filter's operators by label", async () => {
    expect(await openOptions(p({ type: "number", operator: "=" }))).toEqual(
      ["Equal to", "Not equal to", "Between", "Greater than or equal to", "Less than or equal to"]);
  });

  it("offers a date filter's operators by label", async () => {
    expect(await openOptions(p({ type: "date", operator: "range" }))).toEqual(
      ["Date range", "Relative date", "Single date", "Month and year", "Quarter and year", "All options"]);
  });

  it("shows the selected operator's label in the closed select", () => {
    render(<FilterSettingsPanel param={p({ type: "date", operator: "all-options" })} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "parameter operator" }).textContent).toBe("All options");
  });
});
