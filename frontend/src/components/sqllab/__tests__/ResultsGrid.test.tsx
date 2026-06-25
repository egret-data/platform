import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";

test("renders columns, rows, truncation banner, and CSV link", () => {
  render(<ResultsGrid pageIndex={0} onPage={() => {}} truncated csvHref="/api/ws/eng/queries/1/results.csv"
    page={{ columns: [{ name: "n", type: "bigint" }], rows: [[1], [2]], page: 0, page_size: 100, row_count: 2, truncated: true }} />);
  expect(screen.getByText("n")).toBeInTheDocument();
  expect(screen.getByText(/Results were limited/)).toBeInTheDocument();
  expect(screen.getByText("Download CSV").closest("a")).toHaveAttribute("href", "/api/ws/eng/queries/1/results.csv");
});

test("renders the error block when error is set", () => {
  render(<ResultsGrid page={null} pageIndex={0} onPage={() => {}} csvHref={null} truncated={false}
    error={"line 1:8: boom"} statusLine={"Failed in 12 ms"} />);
  expect(screen.getByText("line 1:8: boom")).toBeInTheDocument();
  expect(screen.getByText("Failed in 12 ms")).toBeInTheDocument();
});

test("renders the status line in the footer when results exist", () => {
  const page = { columns: [{ name: "n", type: "int" }], rows: [[1]], page: 0, page_size: 100, row_count: 1, truncated: false };
  render(<ResultsGrid page={page} pageIndex={0} onPage={() => {}} csvHref={null} truncated={false} statusLine={"Done in 1.2 s"} />);
  expect(screen.getByText("Done in 1.2 s")).toBeInTheDocument();
});
