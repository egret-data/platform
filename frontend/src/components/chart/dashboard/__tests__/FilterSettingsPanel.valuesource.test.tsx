import { useState } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

// The onChange spy records every emitted param; the harness also feeds the
// emitted param back as the controlled `param` prop so dependent controls
// (textarea / column select) render after a radio toggle.
function Harness({
  spy,
  cardColumns,
}: {
  spy: (next: DashboardParameter) => void;
  cardColumns?: { cardId: string; label: string; columns: string[] }[];
}) {
  const [param, setParam] = useState<DashboardParameter>(p({}));
  return (
    <FilterSettingsPanel
      param={param}
      allParameters={[p({})]}
      cardColumns={cardColumns}
      onChange={(next) => {
        spy(next);
        setParam(next);
      }}
      onDelete={vi.fn()}
    />
  );
}

describe("FilterSettingsPanel value sources", () => {
  it("Custom list writes a static array (one value per line)", () => {
    const onChange = vi.fn();
    render(<Harness spy={onChange} />);
    fireEvent.click(screen.getByLabelText("values from custom"));
    fireEvent.change(screen.getByLabelText("custom values"), { target: { value: "West\nEast\n" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ valuesSource: { type: "static", static: ["West", "East"] } }));
  });
  it("From a card writes card + column", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Harness spy={onChange} cardColumns={[{ cardId: "c1", label: "Sales", columns: ["region", "city"] }]} />);
    await user.click(screen.getByLabelText("values from card"));
    // Choosing "From a card column" already picks the first card.
    expect(screen.getByRole("combobox", { name: "values card" })).toHaveTextContent("Sales");
    await user.click(screen.getByRole("combobox", { name: "values column" }));
    await user.click(await screen.findByRole("option", { name: "city" }));
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ valuesSource: { type: "card", cardId: "c1", column: "city" } }));
  });
  it("does not offer value sources for a text parameter", () => {
    render(<FilterSettingsPanel param={p({ type: "text", operator: "is" })} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("values from custom")).toBeNull();
  });
});
