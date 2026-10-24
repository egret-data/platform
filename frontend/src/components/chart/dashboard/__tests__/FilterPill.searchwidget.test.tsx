import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterControl } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });
const OPTIONS = ["AFRICA", "AMERICA", "ASIA", "EUROPE"];

// KI-066: "Search box" rendered exactly the Dropdown list.
describe("FilterControl Search box widget (KI-066)", () => {
  it("Dropdown lists every value before anything is typed", () => {
    render(<FilterControl param={p({ widget: "dropdown" })} value={[]} options={OPTIONS} onChange={vi.fn()} />);
    for (const o of OPTIONS) expect(screen.getByLabelText(`option ${o}`)).toBeTruthy();
  });

  it("Search box lists no value until the viewer types, and says so", () => {
    render(<FilterControl param={p({ widget: "search" })} value={[]} options={OPTIONS} onChange={vi.fn()} />);
    for (const o of OPTIONS) expect(screen.queryByLabelText(`option ${o}`)).toBeNull();
    expect(screen.getByText("Type to search the values")).toBeTruthy();
  });

  it("Search box offers the matches for what was typed, and picking one sets it", () => {
    const onChange = vi.fn();
    render(<FilterControl param={p({ widget: "search" })} value={[]} options={OPTIONS} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("search Region"), { target: { value: "am" } });
    expect(screen.getByLabelText("option AMERICA")).toBeTruthy();
    expect(screen.queryByLabelText("option ASIA")).toBeNull();
    fireEvent.click(screen.getByLabelText("option AMERICA"));
    expect(onChange).toHaveBeenCalledWith(["AMERICA"]);
  });

  it("Search box keeps the picked values on show with an empty query, so they can be unpicked", () => {
    const onChange = vi.fn();
    render(<FilterControl param={p({ widget: "search" })} value={["ASIA"]} options={OPTIONS} onChange={onChange} />);
    expect(screen.getByLabelText("option ASIA")).toBeTruthy();
    expect(screen.queryByLabelText("option EUROPE")).toBeNull();
  });
});
