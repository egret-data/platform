import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DrillMenu } from "../DrillMenu";

const ctx = { dims: [{ col: "region", value: "EU" }] };

describe("DrillMenu", () => {
  it("shows a Filter-by item per dim and See-records, fires callbacks", () => {
    const onFilterBy = vi.fn(); const onSeeRecords = vi.fn(); const onClose = vi.fn();
    render(<DrillMenu ctx={ctx} x={10} y={10} onFilterBy={onFilterBy} onSeeRecords={onSeeRecords} onClose={onClose} />);
    fireEvent.click(screen.getByText(/Filter by region is EU/i));
    expect(onFilterBy).toHaveBeenCalledWith([{ col: "region", op: "eq", value: "EU" }]);
    fireEvent.click(screen.getByText(/See these records/i));
    expect(onSeeRecords).toHaveBeenCalledWith([{ col: "region", op: "eq", value: "EU" }]);
  });
  it("with no dims shows only See-records", () => {
    render(<DrillMenu ctx={{ dims: [] }} x={0} y={0} onFilterBy={() => {}} onSeeRecords={() => {}} onClose={() => {}} />);
    expect(screen.queryByText(/Filter by/i)).not.toBeInTheDocument();
    expect(screen.getByText(/See these records/i)).toBeInTheDocument();
  });
});

describe("DrillMenu — bucketed dimensions", () => {
  // "See these records" sent a range while "Filter by" sent an equality, so picking the second
  // narrowed the chart to one day of the month the cell had summed.
  it("filters by the whole bucket and says so", () => {
    const onFilterBy = vi.fn();
    render(
      <DrillMenu
        ctx={{ dims: [{ col: "order_date", value: "2026-01-01", unit: "month" }] }}
        x={0} y={0} onFilterBy={onFilterBy} onSeeRecords={() => {}} onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Filter by order_date ≥ 2026-01-01 and < 2026-02-01"));
    expect(onFilterBy).toHaveBeenCalledWith([
      { col: "order_date", op: "ge", value: "2026-01-01" },
      { col: "order_date", op: "lt", value: "2026-02-01" },
    ]);
  });

  // An extraction unit yields an integer field, not an instant, so `drillFilters` builds nothing
  // for it. Rendering the entry anyway offered a button that silently did nothing.
  it("omits the entry for an extraction unit, which has no range to filter on", () => {
    render(
      <DrillMenu
        ctx={{ dims: [{ col: "order_date", value: 3, unit: "day_of_week" }] }}
        x={0} y={0} onFilterBy={() => {}} onSeeRecords={() => {}} onClose={() => {}}
      />,
    );
    expect(screen.queryByText(/^Filter by/)).toBeNull();
    expect(screen.getByText("See these records")).toBeInTheDocument();
  });
});

// KI-069: the menu opened at the pointer and ran off the right edge for a card on the right.
describe("DrillMenu — stays inside the viewport", () => {
  const menuBox = (width: number, height: number) =>
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      width, height, x: 0, y: 0, top: 0, left: 0, right: width, bottom: height, toJSON: () => ({}),
    } as DOMRect);

  it("moves left and up when the pointer is near the bottom-right corner", () => {
    Object.assign(window, { innerWidth: 1440, innerHeight: 900 });
    const spy = menuBox(192, 103);
    render(<DrillMenu ctx={ctx} x={1340} y={850} onFilterBy={() => {}} onSeeRecords={() => {}} onClose={() => {}} />);
    const menu = screen.getByTestId("drill-menu");
    expect(menu.style.left).toBe(`${1440 - 192 - 8}px`);
    expect(menu.style.top).toBe(`${900 - 103 - 8}px`);
    spy.mockRestore();
  });

  it("stays at the pointer when it fits", () => {
    Object.assign(window, { innerWidth: 1440, innerHeight: 900 });
    const spy = menuBox(192, 103);
    render(<DrillMenu ctx={ctx} x={300} y={200} onFilterBy={() => {}} onSeeRecords={() => {}} onClose={() => {}} />);
    const menu = screen.getByTestId("drill-menu");
    expect(menu.style.left).toBe("300px");
    expect(menu.style.top).toBe("200px");
    spy.mockRestore();
  });
});
