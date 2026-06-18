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
