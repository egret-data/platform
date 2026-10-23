import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "1", name: "Grouping", slug: "grouping", type: "temporal_unit", operator: "is", ...over });

// KI-067: the Type select named the parameter types by their internal keys.
describe("FilterSettingsPanel type labels (KI-067)", () => {
  it("offers every type by a label, never by its internal key", async () => {
    const user = userEvent.setup();
    render(<FilterSettingsPanel param={p({ type: "text" })} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    await user.click(screen.getByRole("combobox", { name: "parameter type" }));
    const opts = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(opts).toEqual(["Text", "Number", "Date", "ID", "Category", "True / False", "Location", "Time grouping"]);
  });

  it("shows the selected type's label in the closed select", () => {
    render(<FilterSettingsPanel param={p()} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "parameter type" }).textContent).toBe("Time grouping");
  });
});
