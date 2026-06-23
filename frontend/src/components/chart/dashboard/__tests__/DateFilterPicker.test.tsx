import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DateFilterPicker } from "../DateFilterPicker";

describe("DateFilterPicker", () => {
  it("relative tab emits a relative value", () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={undefined} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Relative" }));
    fireEvent.change(screen.getByLabelText("relative n"), { target: { value: "7" } });
    fireEvent.change(screen.getByLabelText("relative unit"), { target: { value: "day" } });
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ mode: "relative", dir: "past", n: 7, unit: "day" }),
    );
  });
  it("specific tab between emits from+to", () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={undefined} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Specific" }));
    fireEvent.change(screen.getByLabelText("specific op"), { target: { value: "between" } });
    fireEvent.change(screen.getByLabelText("specific from"), { target: { value: "2026-01-01" } });
    fireEvent.change(screen.getByLabelText("specific to"), { target: { value: "2026-03-31" } });
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" }),
    );
  });
  it("accumulates: editing one field preserves the others (seeded value)", () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={{ mode: "relative", dir: "next", n: 3, unit: "month" }} onChange={onChange} />);
    // change only the unit; dir + n must persist
    fireEvent.change(screen.getByLabelText("relative unit"), { target: { value: "week" } });
    expect(onChange).toHaveBeenLastCalledWith({ mode: "relative", dir: "next", n: 3, unit: "week" });
  });

  it("re-syncs the draft when the value prop changes", () => {
    const onChange = vi.fn();
    const { rerender } = render(<DateFilterPicker value={{ mode: "relative", dir: "past", n: 7, unit: "day" }} onChange={onChange} />);
    // initially the unit select shows "day"
    expect((screen.getByLabelText("relative unit") as HTMLSelectElement).value).toBe("day");
    // parent feeds a new value → control reflects it
    rerender(<DateFilterPicker value={{ mode: "specific", op: "after", from: "2026-05-05" }} onChange={onChange} />);
    expect((screen.getByLabelText("specific from") as HTMLInputElement).value).toBe("2026-05-05");
  });
});
