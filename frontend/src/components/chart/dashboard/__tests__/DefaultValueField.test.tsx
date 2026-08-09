import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DefaultValueField } from "../DefaultValueField";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("DefaultValueField", () => {
  it("picks a category default from the value list, and stores the RAW array", () => {
    // The raw/display trap: a test that asserted the visible text "EU" would pass against an
    // implementation storing the string "EU" — which is not what a multi-value category holds,
    // and not what `paramToFilters` turns into `in [...]`.
    const onChange = vi.fn();
    render(<DefaultValueField param={p({})} options={["EU", "US"]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("option EU"));
    expect(onChange).toHaveBeenCalledWith(["EU"]);
  });

  it("picks a date default with the date picker, not a text box", () => {
    render(<DefaultValueField param={p({ type: "date", operator: "all-options" })} options={[]} onChange={vi.fn()} />);
    expect(screen.getByLabelText("Relative")).toBeTruthy();
    expect(screen.getByLabelText("Specific")).toBeTruthy();
  });

  it("picks a number default with a number input", () => {
    render(<DefaultValueField param={p({ type: "number", operator: "=" })} options={[]} onChange={vi.fn()} />);
    expect((screen.getByLabelText("Region value") as HTMLInputElement).type).toBe("number");
  });

  it("clears to undefined rather than to an empty string", () => {
    // `undefined` is what `useDashboardParams` reads as "no default"; "" would be a default that
    // exists and is blank, and the required-filter notice keys on the difference.
    const onChange = vi.fn();
    render(<DefaultValueField param={p({ default: ["EU"] })} options={["EU"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "clear default value" }));
    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  it("collapses an emptied value from the CONTROL to undefined too, not just the Clear button", () => {
    // The plan asserted this via the "clear default value" button, which calls onChange(undefined)
    // directly and therefore passes with or without the collapse — the mutation survived. The
    // widget's OWN clear emits the type's cleared value ([] for a multi category), and it is that
    // path the collapse exists for. `getByRole(name: "Clear")` picks the inner control's button:
    // the outer one's aria-label overrides its text.
    const onChange = vi.fn();
    render(<DefaultValueField param={p({ default: ["EU"] })} options={["EU"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  it("offers no clear button when there is no default to clear", () => {
    render(<DefaultValueField param={p({})} options={["EU"]} onChange={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "clear default value" })).toBeNull();
  });
});
