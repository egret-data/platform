import { describe, it, expect } from "vitest";
import { render, screen, within, fireEvent } from "@testing-library/react";
import { ChartTable } from "../ChartTable";

const COLS = [{ name: "a", type: "varchar" }, { name: "b", type: "double" }];
const mkRows = (n: number) => Array.from({ length: n }, (_, i) => [`a${i}`, i]);

describe("ChartTable", () => {
  it("renders headers and formatted numeric cells", () => {
    render(
      <ChartTable
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1234.5], ["US", 9876]]}
      />,
    );
    expect(screen.getByText("region")).toBeInTheDocument();
    expect(screen.getByText("revenue")).toBeInTheDocument();
    expect(screen.getByText("1,234.5")).toBeInTheDocument();
  });

  it("formats numeric values delivered as strings", () => {
    render(
      <ChartTable
        columns={[{ name: "revenue", type: "bigint" }]}
        rows={[["1234567"]]}
      />,
    );
    expect(screen.getByText("1,234,567")).toBeInTheDocument();
  });

  it("applies a currency format and a single-color rule", () => {
    render(
      <ChartTable
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1200], ["US", 50]]}
        columnSettings={{ revenue: { number_style: "currency", currency: "USD", currency_style: "code", decimals: 0 } }}
        conditionalFormatting={[{ columns: ["revenue"], type: "single", operator: ">", value: 1000, color: "#ff0000" }]}
      />,
    );
    // currency code uses a non-breaking space (U+00A0) between "USD" and the number
    const formatted = screen.getByText(
      (t) => t.replace(/\u00a0/g, " ") === "USD 1,200",
    );
    expect(formatted).toBeInTheDocument();
    const cell = formatted.closest("td")!;
    expect(cell.style.backgroundColor).toMatch(/rgba\(255, 0, 0/);
  });
});

describe("ChartTable interactions", () => {
  it("hides a column listed in hiddenColumns", () => {
    render(<ChartTable columns={COLS} rows={mkRows(3)} tableState={{ hiddenColumns: ["a"] }} />);
    expect(screen.queryByText("a")).not.toBeInTheDocument();
    expect(screen.getByText("b")).toBeInTheDocument();
  });

  it("ignores stale columnWidths keys for columns that no longer exist", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ columnWidths: { zzz: 200 } }} />);
    expect(screen.getByText("a")).toBeInTheDocument();
    expect(screen.getByText("b")).toBeInTheDocument();
  });

  it("marks the first column sticky when pinnedLeft=1", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ pinnedLeft: 1 }} />);
    const aHeader = screen.getByText("a").closest("th")!;
    expect(aHeader.className).toContain("sticky");
    expect(aHeader.className).toContain("z-30");
  });

  it("paginates to pageSize rows with a working Next button", () => {
    render(<ChartTable columns={COLS} rows={mkRows(30)} tableState={{ pagination: { enabled: true, pageSize: 25 } }} />);
    expect(screen.getByText("Showing 1–25 of 30")).toBeInTheDocument();
    const body = document.querySelector("tbody")!;
    expect(within(body).getAllByRole("row")).toHaveLength(25);
    fireEvent.click(screen.getByText("Next"));
    expect(screen.getByText("Showing 26–30 of 30")).toBeInTheDocument();
  });
});
