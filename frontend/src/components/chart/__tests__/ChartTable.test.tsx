import { describe, it, expect, vi } from "vitest";
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

describe("ChartTable — automatic link detection (Metabase parity)", () => {
  it("auto-links a bare URL in a text column (default view)", () => {
    render(<ChartTable columns={[{ name: "site", type: "varchar" }]} rows={[["https://example.com"]]} />);
    expect(screen.getByRole("link", { name: "https://example.com" })).toHaveAttribute("href", "https://example.com");
  });
  it("auto-links a bare email as mailto", () => {
    render(<ChartTable columns={[{ name: "who", type: "varchar" }]} rows={[["a@b.com"]]} />);
    expect(screen.getByRole("link", { name: "a@b.com" })).toHaveAttribute("href", "mailto:a@b.com");
  });
  it("does NOT auto-link plain text or numbers", () => {
    render(<ChartTable columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]} rows={[["EU", 10]]} />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });
  it("view_as=text disables auto-linking", () => {
    render(<ChartTable columns={[{ name: "site", type: "varchar" }]} rows={[["https://example.com"]]} columnSettings={{ site: { view_as: "text" } }} />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText("https://example.com")).toBeInTheDocument();
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

  it("reorders a column from the keyboard: focus, Space, ArrowRight, Space", async () => {
    // dnd-kit's keyboard sensor picks a target purely from measured DOM rects
    // (getBoundingClientRect, normally kept fresh by ResizeObserver). jsdom returns an
    // all-zero rect for everything and the project's ResizeObserver shim is a no-op
    // (vitest.setup.ts), so without this, every header collapses to the same rect and no
    // direction ever reads as "further right" — give each <th> a distinct, ordered rect so
    // the sensor has real geometry to act on, exactly as it would in a browser.
    const orig = HTMLElement.prototype.getBoundingClientRect;
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
      if (this.tagName === "TH") {
        const row = this.closest("tr");
        const ths = row ? Array.from(row.querySelectorAll<HTMLElement>("th")) : [];
        const i = Math.max(0, ths.indexOf(this));
        const left = i * 100;
        return { x: left, y: 0, left, top: 0, right: left + 100, bottom: 30, width: 100, height: 30, toJSON() {} } as DOMRect;
      }
      return orig.call(this);
    };
    try {
      const onTableStateChange = vi.fn();
      render(<ChartTable columns={COLS} rows={mkRows(2)} onTableStateChange={onTableStateChange} />);
      const header = screen.getByText("a").closest("th")!;
      // Focusable: dnd-kit's useSortable attributes (tabIndex from `attributes`) are spread
      // onto the header, same element the click-to-sort handler lives on.
      expect(header).toHaveAttribute("tabIndex", "0");
      header.focus();
      expect(header).toHaveFocus();
      fireEvent.keyDown(header, { code: "Space", key: " " });
      // KeyboardSensor.attach() wires its real keydown listener via a setTimeout(0) so it
      // doesn't steal the very keydown that activated it; yield a macrotask so it's attached
      // before the next key, same as it would be after any real gap between keypresses.
      await new Promise((r) => setTimeout(r, 0));
      fireEvent.keyDown(header, { code: "ArrowRight", key: "ArrowRight" });
      fireEvent.keyDown(header, { code: "Space", key: " " });
      expect(onTableStateChange).toHaveBeenCalledWith(expect.objectContaining({ columnOrder: ["b", "a"] }));
    } finally {
      HTMLElement.prototype.getBoundingClientRect = orig;
    }
  });

  it("clicking a header still sorts after the keyboard sensor is wired up (no cross-talk)", () => {
    render(<ChartTable columns={COLS} rows={mkRows(3)} />);
    const header = screen.getByText("b").closest("th")!;
    fireEvent.click(header);
    const firstRowAsc = within(document.querySelector("tbody")!).getAllByRole("row")[0];
    expect(firstRowAsc).toHaveTextContent("a0");
  });
});

describe("ChartTable — every column hidden", () => {
  it("shows a message instead of an empty grid when every column is hidden", () => {
    render(<ChartTable columns={COLS} rows={mkRows(3)} tableState={{ hiddenColumns: ["a", "b"] }} />);
    expect(screen.getByText(/every column is hidden/i)).toBeInTheDocument();
    expect(document.querySelector("table")).not.toBeInTheDocument();
    expect(document.querySelector("th")).not.toBeInTheDocument();
    expect(document.querySelector("td")).not.toBeInTheDocument();
  });
});

describe("ChartTable — column_title header rename", () => {
  const REV_COLS = [{ name: "region", type: "varchar" }, { name: "total_revenue", type: "double" }];
  const REV_ROWS = [["EU", 30], ["US", 10], ["APAC", 20]];

  it("shows column_title as the header instead of the raw name", () => {
    render(
      <ChartTable
        columns={REV_COLS}
        rows={REV_ROWS}
        columnSettings={{ total_revenue: { column_title: "Revenue" } }}
      />,
    );
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByText("total_revenue")).not.toBeInTheDocument();
  });

  it("still sorts by clicking a renamed header", () => {
    render(
      <ChartTable
        columns={REV_COLS}
        rows={REV_ROWS}
        columnSettings={{ total_revenue: { column_title: "Revenue" } }}
      />,
    );
    const header = screen.getByText("Revenue");
    // Ascending on the first click, for numbers as well as text — Metabase's convention, and
    // `sortDescFirst: false` is what holds TanStack's numeric columns to it.
    fireEvent.click(header);
    const asc = within(document.querySelector("tbody")!).getAllByRole("row");
    expect(asc[0]).toHaveTextContent("US");
    expect(asc[0]).toHaveTextContent("10");
    fireEvent.click(header);
    const desc = within(document.querySelector("tbody")!).getAllByRole("row");
    expect(desc[0]).toHaveTextContent("EU");
    expect(desc[0]).toHaveTextContent("30");
  });

  it("falls back to the raw name for a column with no column_title, alongside a renamed one", () => {
    render(
      <ChartTable
        columns={REV_COLS}
        rows={REV_ROWS}
        columnSettings={{ total_revenue: { column_title: "Revenue" } }}
      />,
    );
    expect(screen.getByText("region")).toBeInTheDocument();
    expect(screen.getByText("Revenue")).toBeInTheDocument();
  });
});

describe("cell drill", () => {
  // A wiring test, not a logic one: `tableCellDrill` had always accepted `temporalUnits` and
  // ChartTable had always failed to pass it, so every unit test of the drill helper passed while
  // the table on screen still filtered by the instant. Assert the prop actually arrives.
  it("passes the temporal unit of a bucketed dimension into the drill context", () => {
    const onCellDrill = vi.fn();
    render(
      <ChartTable
        columns={[{ name: "order_date", type: "date" }, { name: "revenue", type: "double" }]}
        rows={[["2026-01-01", 2084]]}
        dimensionColumns={["order_date"]}
        temporalUnits={{ order_date: "month" }}
        onCellDrill={onCellDrill}
      />,
    );
    fireEvent.click(screen.getByText("2,084"));
    expect(onCellDrill).toHaveBeenCalled();
    expect(onCellDrill.mock.calls[0][0].dims).toEqual([
      { col: "order_date", value: "2026-01-01", unit: "month" },
    ]);
  });
});

// dnd-kit marks an undraggable item `aria-disabled`, and pinning a column makes it undraggable.
// Spread onto the <th> that meant a frozen column — and, by state propagation, its resize
// separator — was announced as unavailable even though clicking sorted it and dragging resized it.
it("does not announce a pinned column header as disabled", () => {
  render(
    <ChartTable
      columns={[{ name: "order_date", type: "date" }, { name: "revenue", type: "double" }]}
      rows={[["2026-01-01", 2084]]}
      tableState={{ pinnedLeft: 1 }}
    />,
  );
  const th = screen.getByRole("columnheader", { name: /order_date/ });
  expect(th).not.toHaveAttribute("aria-disabled");
  expect(within(th).getByRole("separator", { name: "Resize column" })).not.toHaveAttribute("aria-disabled");
});

describe("row index", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }];
  const rows = [["North", 3], ["South", 1], ["East", 2]];

  it("is absent unless asked for", () => {
    render(<ChartTable columns={cols} rows={rows} />);
    expect(screen.queryByRole("columnheader", { name: "#" })).toBeNull();
  });

  // Numbers the rows AS DISPLAYED. Using the row's index into the source data would restart the
  // count from wherever a sorted row originally sat, so the column would read 3, 1, 2.
  it("numbers rows by their position on screen, not their position in the data", () => {
    render(<ChartTable columns={cols} rows={rows} tableState={{ rowIndex: true }} />);
    fireEvent.click(screen.getByRole("columnheader", { name: /revenue/ }));
    const body = screen.getAllByRole("row").slice(1) as HTMLTableRowElement[];
    expect(body.map((r) => r.cells[0].textContent)).toEqual(["1", "2", "3"]);
    expect(body.map((r) => r.cells[2].textContent)).toEqual(["1", "2", "3"]);
  });
});
