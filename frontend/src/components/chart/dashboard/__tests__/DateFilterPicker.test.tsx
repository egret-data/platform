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
});
