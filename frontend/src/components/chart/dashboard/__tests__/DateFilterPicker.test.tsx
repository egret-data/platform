import { beforeAll, describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

/** Opens the shared select named `name` and picks the item named `option`. */
async function pick(name: string, option: string) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("combobox", { name }));
  await user.click(await screen.findByRole("option", { name: option }));
}
import { DateFilterPicker } from "../DateFilterPicker";

// The date boxes are the app's calendar (KI-276); its month grid loads lazily, so load it once here.
beforeAll(async () => {
  await import("@/components/ui/calendar");
}, 30_000);

const pad = (n: number) => String(n).padStart(2, "0");
const now = new Date();
const YM = `${now.getFullYear()}-${pad(now.getMonth() + 1)}`;

/** Opens the date picker named `name` and picks the `day`th of this month. */
async function pickDay(name: string, day: "1st" | "15th") {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name }));
  const grid = await screen.findByRole("grid");
  // The grid also shows the next month's first days, so the first match is this month's.
  await user.click(within(grid).getAllByRole("button", { name: new RegExp(`\\b${day}\\b`) })[0]);
}

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
    await pickDay("specific from", "1st");
    await pickDay("specific to", "15th");
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ mode: "specific", op: "between", from: `${YM}-01`, to: `${YM}-15` }),
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
    expect(screen.getByRole("button", { name: "specific from" })).toHaveTextContent("2026-05-05");
  });
});
