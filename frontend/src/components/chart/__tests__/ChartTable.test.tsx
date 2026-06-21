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

describe("ChartTable — view-as (Metabase parity)", () => {
  it("renders a link with default href = value", () => {
    render(
      <ChartTable
        columns={[{ name: "site", type: "varchar" }]}
        rows={[["https://example.com"]]}
        columnSettings={{ site: { view_as: "link" } }}
      />,
    );
    const a = screen.getByRole("link", { name: "https://example.com" });
    expect(a).toHaveAttribute("href", "https://example.com");
    expect(a).toHaveAttribute("target", "_blank");
  });
  it("applies link_text and link_url templates with {{value}}", () => {
    render(
      <ChartTable
        columns={[{ name: "id", type: "varchar" }]}
        rows={[["42"]]}
        columnSettings={{ id: { view_as: "link", link_text: "Order {{value}}", link_url: "/orders/{{value}}" } }}
      />,
    );
    const a = screen.getByRole("link", { name: "Order 42" });
    expect(a).toHaveAttribute("href", "/orders/42");
  });
  it("renders an email as a mailto link", () => {
    render(
      <ChartTable
        columns={[{ name: "email", type: "varchar" }]}
        rows={[["a@b.com"]]}
        columnSettings={{ email: { view_as: "email" } }}
      />,
    );
    expect(screen.getByRole("link", { name: "a@b.com" })).toHaveAttribute("href", "mailto:a@b.com");
  });
  it("renders an image with the value as src", () => {
    render(
      <ChartTable
        columns={[{ name: "img", type: "varchar" }]}
        rows={[["/logo.png"]]}
        columnSettings={{ img: { view_as: "image" } }}
      />,
    );
    expect(document.querySelector("img")).toHaveAttribute("src", "/logo.png");
  });
});

describe("ChartTable — mini bar (Metabase parity)", () => {
  it("draws an in-cell bar alongside the formatted number", () => {
    render(
      <ChartTable
        columns={[{ name: "rev", type: "double" }]}
        rows={[[20], [80], [100]]}
        columnSettings={{ rev: { show_mini_bar: true } }}
      />,
    );
    // number still shows
    expect(screen.getByText("80")).toBeInTheDocument();
    const bars = document.querySelectorAll('[data-testid="mini-bar"]');
    expect(bars.length).toBe(3);
    // the 100 row fills the track fully; the 20 row is a fifth of it
    const widths = Array.from(bars).map((b) => (b as HTMLElement).style.width);
    expect(widths).toContain("100%");
    expect(widths).toContain("20%");
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

  it("renders columns in tableState.columnOrder", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ columnOrder: ["b", "a"] }} />);
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers[0]).toHaveTextContent("b");
    expect(headers[1]).toHaveTextContent("a");
  });
});
