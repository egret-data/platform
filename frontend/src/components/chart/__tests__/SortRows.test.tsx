import { useState } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ChartOrderBy } from "@/api/charts";
import { SortRows } from "../SortRows";

/** A live host, so every assertion below goes through the same state round-trip the studio does. */
function Host({ fields, initial = [] }: { fields: string[]; initial?: ChartOrderBy[] }) {
  const [value, setValue] = useState<ChartOrderBy[]>(initial);
  return (
    <>
      <SortRows fields={fields} value={value} onChange={setValue} />
      <output data-testid="state">{JSON.stringify(value)}</output>
    </>
  );
}

const state = () => JSON.parse(screen.getByTestId("state").textContent!) as ChartOrderBy[];

/** The bar chart's sortable fields: a breakout and an aggregation (`orderby_sql`'s first two arms). */
const AGGREGATE_FIELDS = ["category", "revenue"];
/** The box plot's: raw columns (`orderby_sql`'s third arm). */
const RAW_FIELDS = ["category", "score"];

describe("SortRows", () => {
  it("renders nothing when the spec has no sortable field", () => {
    const { container } = render(<SortRows fields={[]} value={[]} onChange={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("offers exactly the aggregate spec's fields, and adds ascending like Metabase", async () => {
    render(<Host fields={AGGREGATE_FIELDS} />);
    await userEvent.click(screen.getByRole("combobox", { name: /add a sort/i }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["category", "revenue"]);
    await userEvent.click(screen.getByRole("option", { name: "revenue" }));
    // Metabase's `handleAddOrderBy` calls `Lib.orderBy(..., "asc")` — a new clause is ascending.
    expect(state()).toEqual([{ field: "revenue", desc: false }]);
  });

  it("offers exactly the raw-records spec's columns", async () => {
    render(<Host fields={RAW_FIELDS} />);
    await userEvent.click(screen.getByRole("combobox", { name: /add a sort/i }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["category", "score"]);
    await userEvent.click(screen.getByRole("option", { name: "score" }));
    expect(state()).toEqual([{ field: "score", desc: false }]);
  });

  it("does not offer a field already sorted on, and drops the add picker once all are used", async () => {
    render(<Host fields={AGGREGATE_FIELDS} initial={[{ field: "revenue", desc: true }]} />);
    await userEvent.click(screen.getByRole("combobox", { name: /add a sort/i }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["category"]);
    await userEvent.click(screen.getByRole("option", { name: "category" }));
    expect(state()).toEqual([{ field: "revenue", desc: true }, { field: "category", desc: false }]);
    expect(screen.queryByRole("combobox", { name: /add a sort/i })).not.toBeInTheDocument();
  });

  it("lets a clause change its own field, and still offers the one it already has", async () => {
    render(<Host fields={AGGREGATE_FIELDS} initial={[{ field: "revenue", desc: true }]} />);
    await userEvent.click(screen.getByRole("combobox", { name: /sort 1 field/i }));
    // Metabase's picker filter is `isSelected || !isAlreadyUsed`: this clause's own column stays.
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["category", "revenue"]);
    await userEvent.click(screen.getByRole("option", { name: "category" }));
    // The direction survives the field change.
    expect(state()).toEqual([{ field: "category", desc: true }]);
  });

  it("toggles direction both ways, and says which way in the DOM", async () => {
    render(<Host fields={AGGREGATE_FIELDS} initial={[{ field: "revenue", desc: false }]} />);
    await userEvent.click(screen.getByRole("button", { name: /sort 1 direction: ascending/i }));
    expect(state()).toEqual([{ field: "revenue", desc: true }]);
    await userEvent.click(screen.getByRole("button", { name: /sort 1 direction: descending/i }));
    expect(state()).toEqual([{ field: "revenue", desc: false }]);
  });

  it("removes the clause the button belongs to, not the first one", async () => {
    render(<Host fields={AGGREGATE_FIELDS}
      initial={[{ field: "revenue", desc: true }, { field: "category", desc: false }]} />);
    await userEvent.click(screen.getByRole("button", { name: /remove sort 2/i }));
    expect(state()).toEqual([{ field: "revenue", desc: true }]);
  });

  it("renders one row per clause IN ARRAY ORDER — the array's order is the precedence", () => {
    render(<Host fields={AGGREGATE_FIELDS}
      initial={[{ field: "revenue", desc: true }, { field: "category", desc: false }]} />);
    const rows = screen.getAllByRole("combobox", { name: /sort \d field/i });
    expect(rows.map((r) => r.textContent)).toEqual(["revenue", "category"]);
  });

  it("reorders with the arrows, and pins the ends", async () => {
    render(<Host fields={AGGREGATE_FIELDS}
      initial={[{ field: "revenue", desc: true }, { field: "category", desc: false }]} />);
    expect(screen.getByRole("button", { name: /move sort 1 up/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /move sort 2 down/i })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: /move sort 2 up/i }));
    expect(state()).toEqual([{ field: "category", desc: false }, { field: "revenue", desc: true }]);
    await userEvent.click(screen.getByRole("button", { name: /move sort 1 down/i }));
    expect(state()).toEqual([{ field: "revenue", desc: true }, { field: "category", desc: false }]);
  });

  it("drops a stored clause whose field the spec no longer offers, rather than sending it again", async () => {
    // Change a bar chart's x-axis and the sort saved on the old one names a field the query no
    // longer selects — `orderby_sql` answers that with a 400, so the control must not keep it.
    const onChange = vi.fn();
    render(<SortRows fields={AGGREGATE_FIELDS} value={[{ field: "gone", desc: true }]} onChange={onChange} />);
    expect(onChange).toHaveBeenCalledWith([]);
  });
});

// KI-050: the field menu names each field the way the field pickers do — its display name — while
// the clause still stores the machine name.
describe("SortRows — display names", () => {
  it("lists display names and stores machine names", async () => {
    function Labelled() {
      const [value, setValue] = useState<ChartOrderBy[]>([]);
      return (
        <>
          <SortRows fields={AGGREGATE_FIELDS} labels={{ revenue: "Revenue" }} value={value} onChange={setValue} />
          <output data-testid="state">{JSON.stringify(value)}</output>
        </>
      );
    }
    render(<Labelled />);
    await userEvent.click(screen.getByRole("combobox", { name: /add a sort/i }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["category", "Revenue"]);
    await userEvent.click(screen.getByRole("option", { name: "Revenue" }));
    expect(state()).toEqual([{ field: "revenue", desc: false }]);
    expect(screen.getByRole("combobox", { name: "sort 1 field" })).toHaveTextContent("Revenue");
  });
});
