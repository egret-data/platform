import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p: DashboardParameter = { id: "1", name: "When", slug: "when", type: "date", operator: "all-options" };

describe("FilterPill all-options", () => {
  it("opens the DateFilterPicker and emits a relative value", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p} value={undefined} options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "When" }));
    fireEvent.click(screen.getByRole("button", { name: "Relative" }));
    fireEvent.change(screen.getByLabelText("relative n"), { target: { value: "3" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "relative", n: 3 }));
  });
});
