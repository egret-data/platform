import { render, screen, fireEvent } from "@testing-library/react";
import { expect, test } from "vitest";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import type { ResultsPage } from "@/api/sqllab";

const mk = (rows: unknown[][]): ResultsPage => ({
  columns: [{ name: "n", type: "bigint" }, { name: "label", type: "varchar" }],
  rows, page: 0, page_size: 1000, row_count: rows.length, truncated: false,
});

test("renders columns, rows and total row count", () => {
  render(<ResultsGrid page={mk([[2, "b"], [1, "a"]])} truncated={false} csvHref={null} />);
  expect(screen.getByText("n")).toBeInTheDocument();
  expect(screen.getByText("label")).toBeInTheDocument();
  expect(screen.getByText("2 rows")).toBeInTheDocument();
});

test("renders the truncation banner", () => {
  render(<ResultsGrid truncated csvHref="/api/ws/eng/queries/1/results.csv"
    page={{ columns: [{ name: "n", type: "bigint" }], rows: [[1], [2]], page: 0, page_size: 1000, row_count: 2, truncated: true }} />);
  expect(screen.getByText(/Results were limited/)).toBeInTheDocument();
});

test("renders the error block when error is set", () => {
  render(<ResultsGrid page={null} csvHref={null} truncated={false} error={"line 1:8: boom"} statusLine={"Failed in 12 ms"} />);
  expect(screen.getByText("line 1:8: boom")).toBeInTheDocument();
  expect(screen.getByText("Failed in 12 ms")).toBeInTheDocument();
});

test("renders the status line in the footer when results exist", () => {
  render(<ResultsGrid page={mk([[1, "a"]])} csvHref={null} truncated={false} statusLine={"Done in 1.2 s"} />);
  expect(screen.getByText("Done in 1.2 s")).toBeInTheDocument();
});

test("clicking a column header toggles aria-sort", () => {
  render(<ResultsGrid page={mk([[2, "b"], [1, "a"]])} truncated={false} csvHref={null} />);
  const th = screen.getByText("n").closest("th")!;
  expect(th).toHaveAttribute("aria-sort", "none");
  fireEvent.click(screen.getByRole("button", { name: "n" }));
  expect(th).toHaveAttribute("aria-sort", "ascending");
  fireEvent.click(screen.getByRole("button", { name: "n" }));
  expect(th).toHaveAttribute("aria-sort", "descending");
});

test("filtering rows narrows the set and shows the match count", () => {
  render(<ResultsGrid page={mk([[1, "apple"], [2, "banana"]])} truncated={false} csvHref={null} />);
  fireEvent.change(screen.getByLabelText("Filter result rows"), { target: { value: "banana" } });
  expect(screen.getByText("banana")).toBeInTheDocument();
  expect(screen.queryByText("apple")).not.toBeInTheDocument();
  expect(screen.getByText("1 of 2")).toBeInTheDocument();
});

/** KI-004: doubles are shown without binary representation noise; exact types are untouched. */
test("rounds floating-point cells for display only, and leaves exact types and the copied value alone", () => {
  const page: ResultsPage = {
    columns: [
      { name: "d", type: "double" }, { name: "r", type: "real" }, { name: "dec", type: "decimal(38,2)" },
      { name: "big", type: "bigint" }, { name: "nan", type: "double" },
    ],
    rows: [[63339475.32000001, 0.30000000000000004, "0.30000000000000004", 9007199254740991, "NaN"]],
    page: 0, page_size: 1000, row_count: 1, truncated: false,
  };
  render(<ResultsGrid page={page} truncated={false} csvHref={null} />);
  expect(screen.getByText("63339475.32")).toBeInTheDocument();
  expect(screen.getByText("0.3")).toBeInTheDocument();
  // A decimal arrives as a string and is exact: shown as sent.
  expect(screen.getByText("0.30000000000000004")).toBeInTheDocument();
  expect(screen.getByText(String(9007199254740991))).toBeInTheDocument();
  expect(screen.getByText("NaN")).toBeInTheDocument();
  // The expanded cell shows the exact value.
  fireEvent.click(screen.getByText("63339475.32"));
  expect(screen.getByRole("dialog")).toHaveTextContent("63339475.32000001");
});
