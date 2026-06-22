import { describe, it, expect, vi } from "vitest";
import { useState } from "react";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

/** Controlled wrapper that holds value in state and feeds it back via onChange,
 *  mirroring how the real dashboard drives the pill. */
function ControlledPill({
  param,
  initial,
  opts,
  onChange,
}: {
  param: DashboardParameter;
  initial: unknown;
  opts: string[];
  onChange?: (v: unknown) => void;
}) {
  const [value, setValue] = useState<unknown>(initial);
  return (
    <FilterPill
      param={param}
      value={value}
      options={opts}
      onChange={(v) => {
        setValue(v);
        onChange?.(v);
      }}
    />
  );
}

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "is", ...over });

const openPill = (param: DashboardParameter, value: unknown, opts: string[], onChange = vi.fn()) => {
  render(<FilterPill param={param} value={value} options={opts} onChange={onChange} />);
  fireEvent.click(screen.getByRole("button", { name: param.name }));
  return onChange;
};

describe("FilterPill tier2d", () => {
  it("boolean renders a 3-state Any/True/False select and encodes the choice", () => {
    const onChange = openPill(p({ type: "boolean", operator: "is" }), "", []);
    const sel = screen.getByLabelText("F value") as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(["", "true", "false"]);
    fireEvent.change(sel, { target: { value: "true" } });
    expect(onChange).toHaveBeenCalledWith("true");
  });

  it("widget=input renders a text input for a category and writes a scalar string", () => {
    const onChange = openPill(p({ type: "category", operator: "is", widget: "input" }), "", ["a", "b"]);
    const input = screen.getByLabelText("F value") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "hello" } });
    expect(onChange).toHaveBeenCalledWith("hello");
  });

  it("location renders a multi-checkbox list", () => {
    const onChange = openPill(p({ type: "location", operator: "is" }), [], ["US", "CA"]);
    fireEvent.click(screen.getByLabelText("option US").querySelector("button,[role=checkbox]") ?? screen.getByLabelText("option US"));
    expect(onChange).toHaveBeenCalledWith(["US"]);
  });

  it("month-year renders an <input type=month>", () => {
    const onChange = openPill(p({ type: "date", operator: "month-year" }), "", []);
    const input = screen.getByLabelText("F value") as HTMLInputElement;
    expect(input.type).toBe("month");
    fireEvent.change(input, { target: { value: "2024-03" } });
    expect(onChange).toHaveBeenCalledWith("2024-03");
  });

  it("quarter-year renders a year input + a quarter select and encodes 'YYYY-Q'", () => {
    const param = p({ type: "date", operator: "quarter-year" });
    const onChange = vi.fn();
    render(<ControlledPill param={param} initial="" opts={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: param.name }));
    fireEvent.change(screen.getByLabelText("F year"), { target: { value: "2024" } });
    expect(onChange).toHaveBeenCalledWith("2024-1");
    fireEvent.change(screen.getByLabelText("F quarter"), { target: { value: "3" } });
    expect(onChange).toHaveBeenLastCalledWith("2024-3");
  });

  it("quarter-year re-seeds local state when value is cleared externally while the popover stays open", () => {
    const param = p({ type: "date", operator: "quarter-year" });
    const onChange = vi.fn();
    render(<ControlledPill param={param} initial="2024-3" opts={[]} onChange={onChange} />);
    // Open the popover; the year input shows the seeded year.
    fireEvent.click(screen.getByRole("button", { name: param.name }));
    expect((screen.getByLabelText("F year") as HTMLInputElement).value).toBe("2024");
    // Clear via the sibling X button — value becomes "" but the popover stays open.
    fireEvent.click(screen.getByRole("button", { name: `clear ${param.name}` }));
    expect(onChange).toHaveBeenLastCalledWith("");
    // The re-seed effect must wipe the local year so it no longer reads "2024".
    expect((screen.getByLabelText("F year") as HTMLInputElement).value).toBe("");
    // And the next quarter edit must NOT republish the stale "2024".
    onChange.mockClear();
    fireEvent.change(screen.getByLabelText("F quarter"), { target: { value: "2" } });
    expect(onChange).not.toHaveBeenCalledWith("2024-2");
  });
});
