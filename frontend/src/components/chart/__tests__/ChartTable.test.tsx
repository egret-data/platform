import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ChartTable } from "../ChartTable";

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
