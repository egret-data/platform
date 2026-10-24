import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { ChartTable } from "../ChartTable";
import { ObjectDetail } from "../ObjectDetail";
import { exportCell } from "../export-chart";
import { isIdentifierColumn } from "../format";
import { FormatSection } from "../settings/FormatSection";

/**
 * KI-058: identifier columns (`orderkey`, `custkey`) were formatted as quantities — 29,989 — in
 * tables and Object detail. An identifier is recognised by an INTEGER type and a name that says
 * so; it then has no thousands separator by default, and a separator style the user chose (on the
 * chart or in the dataset) still wins.
 */
describe("isIdentifierColumn", () => {
  it.each([
    ["id", "bigint"], ["orderkey", "bigint"], ["custkey", "integer"], ["order_key", "int"],
    ["customer_id", "bigint"], ["CUSTOMER_ID", "BIGINT"], ["userId", "int"], ["nationkey", "smallint"],
  ])("%s (%s) is an identifier", (name, type) => {
    expect(isIdentifierColumn(name, type)).toBe(true);
  });

  it.each([
    ["totalprice", "double"],   // not a name that says identifier
    ["orderkey", "double"],     // not an integer: a measure that happens to be named so
    ["orderkey", "varchar"],    // already text, nothing to format
    ["paid", "bigint"],         // ends in "id" but is not `_id` / `Id`
    ["quantity", "bigint"],
    ["shippriority", "integer"],
  ])("%s (%s) is not", (name, type) => {
    expect(isIdentifierColumn(name, type)).toBe(false);
  });
});

const cols = [{ name: "orderkey", type: "bigint" }, { name: "totalprice", type: "double" }];
const rows = [[29989, 28062.96]];

describe("identifier cells are drawn without thousands separators", () => {
  it("in a table", () => {
    render(<ChartTable columns={cols} rows={rows} />);
    expect(screen.getByText("29989")).toBeInTheDocument();
    expect(screen.getByText("28,062.96")).toBeInTheDocument();
  });

  it("in Object detail", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    expect(screen.getByText("29989")).toBeInTheDocument();
    expect(screen.getByText("28,062.96")).toBeInTheDocument();
  });

  it("in a formatted export, which matches the screen", () => {
    expect(exportCell(29989, cols[0], {}, true)).toBe("29989");
    expect(exportCell(28062.96, cols[1], {}, true)).toBe("28,062.96");
  });

  it("an explicit separator style wins", () => {
    render(<ChartTable columns={cols} rows={rows} columnSettings={{ orderkey: { number_separators: ".," } }} />);
    expect(screen.getByText("29,989")).toBeInTheDocument();
  });
});

describe("the Separator style control shows what an identifier is drawn with", () => {
  it("reads 100000.00 for an identifier with nothing stored, and 100,000.00 for a measure", () => {
    const { unmount } = render(<FormatSection columns={[{ name: "orderkey", type: "bigint" }]} value={{}} onChange={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /orderkey/ }));
    expect(screen.getByLabelText("Separator style")).toHaveTextContent("100000.00");
    unmount();
    render(<FormatSection columns={[{ name: "totalprice", type: "bigint" }]} value={{}} onChange={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /totalprice/ }));
    expect(screen.getByLabelText("Separator style")).toHaveTextContent("100,000.00");
  });
});
