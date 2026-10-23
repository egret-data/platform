import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

/** Opens the shared select named `name` and picks the item named `option`. */
async function pick(name: string, option: string) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("combobox", { name }));
  await user.click(await screen.findByRole("option", { name: option }));
}
import { DateFilterPicker } from "../DateFilterPicker";

describe("DateFilterPicker", () => {
  it("relative tab emits a relative value", async () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={undefined} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Relative" }));
    fireEvent.change(screen.getByLabelText("relative n"), { target: { value: "7" } });
    await pick("relative unit", "weeks");
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ mode: "relative", dir: "past", n: 7, unit: "week" }),
    );
  });
  it("specific tab between emits from+to", async () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={undefined} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Specific" }));
    await pick("specific op", "Between");
    fireEvent.change(screen.getByLabelText("specific from"), { target: { value: "2026-01-01" } });
    fireEvent.change(screen.getByLabelText("specific to"), { target: { value: "2026-03-31" } });
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" }),
    );
  });
  it("accumulates: editing one field preserves the others (seeded value)", async () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={{ mode: "relative", dir: "next", n: 3, unit: "month" }} onChange={onChange} />);
    // change only the unit; dir + n must persist
    await pick("relative unit", "weeks");
    expect(onChange).toHaveBeenLastCalledWith({ mode: "relative", dir: "next", n: 3, unit: "week" });
  });

  it("re-syncs the draft when the value prop changes", () => {
    const onChange = vi.fn();
    const { rerender } = render(<DateFilterPicker value={{ mode: "relative", dir: "past", n: 7, unit: "day" }} onChange={onChange} />);
    // initially the unit select shows "day"
    expect(screen.getByRole("combobox", { name: "relative unit" })).toHaveTextContent("days");
    // parent feeds a new value → control reflects it
    rerender(<DateFilterPicker value={{ mode: "specific", op: "after", from: "2026-05-05" }} onChange={onChange} />);
    expect((screen.getByLabelText("specific from") as HTMLInputElement).value).toBe("2026-05-05");
  });
});
