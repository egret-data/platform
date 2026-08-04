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

  /**
   * "FREEZE FIRST COLUMNS" COUNTS IN DISPLAY ORDER, WHICH IS THE ONLY ORDER THE USER CAN SEE.
   *
   * `columnPinning.left` used to be built as `col_0 … col_{n-1}` — ids assigned by RESULT index —
   * while `columnOrder` reorders those same ids for display. With both set they disagreed, and
   * because tanstack draws pinned columns first, freezing after a drag silently UNDID the drag:
   * dragging `b` to the front and freezing one column put `a` back on the left, frozen.
   *
   * Both halves are asserted. That the right column is frozen is not enough on its own — a build
   * that froze nothing would satisfy "`a` is not sticky" — so the draw order is pinned too.
   */
  it("freezes the first column in DISPLAY order, not the first the result happened to return", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ columnOrder: ["b", "a"], pinnedLeft: 1 }} />);
    // `z-30`, not `sticky`: EVERY header is `sticky top-0` — that is the header ROW staying put
    // under a vertical scroll, nothing to do with pinning. The z-index is what actually differs
    // (`z-30` pinned, `z-10` not), which is why the existing pinnedLeft test asserts on it too.
    expect(screen.getByText("b").closest("th")!.className).toContain("z-30");
    expect(screen.getByText("a").closest("th")!.className).toContain("z-10");
    // …and the reorder survives the freeze rather than being undone by it.
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers.map((h) => h.textContent)).toEqual(["b", "a"]);
  });

  /**
   * A `columnOrder` naming only some of the result's columns is the normal state — `ChartTable`
   * appends whatever it does not mention. The freeze has to count down the SAME appended list, or
   * a partial order re-points it to a column the user never froze.
   */
  it("counts the freeze down the appended tail of a partial columnOrder", () => {
    const three = [...COLS, { name: "c", type: "varchar" }];
    render(
      <ChartTable
        columns={three}
        rows={[["a0", 0, "c0"]]}
        tableState={{ columnOrder: ["c"], pinnedLeft: 2 }}
      />,
    );
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers.map((h) => h.textContent)).toEqual(["c", "a", "b"]);
    expect(headers[0].className).toContain("z-30");
    expect(headers[1].className).toContain("z-30");
    expect(headers[2].className).toContain("z-10");
  });

  /**
   * …and it skips a hidden column rather than spending the count on it. A pinned id that
   * `columnVisibility` has switched off is inert in tanstack — it never reaches
   * `getVisibleLeafColumns` — so counting it meant "freeze 1" froze nothing at all whenever the
   * leading column was hidden.
   */
  it("spends the freeze on visible columns, not on a hidden one it cannot draw", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ hiddenColumns: ["a"], pinnedLeft: 1 }} />);
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers.map((h) => h.textContent)).toEqual(["b"]);
    expect(headers[0].className).toContain("z-30");
  });

  it("paginates to pageSize rows with a working Next button", () => {
    render(<ChartTable columns={COLS} rows={mkRows(30)} tableState={{ pagination: { enabled: true, pageSize: 25 } }} />);
    expect(screen.getByText("1–25 of 30")).toBeInTheDocument();
    const body = document.querySelector("tbody")!;
    expect(within(body).getAllByRole("row")).toHaveLength(25);
    fireEvent.click(screen.getByText("Next"));
    expect(screen.getByText("26–30 of 30")).toBeInTheDocument();
  });

  /**
   * The row count is not a pagination detail. Metabase prints one under EVERY table —
   * `getRowCountMessage` / `QuestionRowCount` both render `Showing ${formatRowCount(n)}`, and
   * `formatRowCount` is an `ngettext` over "row"/"rows" — while the page range (`1 - 10 of 100`,
   * `PaginationControls`) is a separate thing that only appears when there is more than one page.
   *
   * egret had the count nested inside `{paginationEnabled && …}`, and pagination is off by default,
   * so a 60-row table said nothing at all about how many rows it was showing.
   */
  it("prints the row count on an unpaginated table, which is every table by default", () => {
    render(<ChartTable columns={COLS} rows={mkRows(60)} />);
    expect(screen.getByText("Showing 60 rows")).toBeInTheDocument();
  });

  it("says row, not rows, when there is one — Metabase's count is an ngettext", () => {
    render(<ChartTable columns={COLS} rows={mkRows(1)} />);
    expect(screen.getByText("Showing 1 row")).toBeInTheDocument();
  });

  it("still counts an empty result rather than going silent", () => {
    render(<ChartTable columns={COLS} rows={[]} />);
    expect(screen.getByText("Showing 0 rows")).toBeInTheDocument();
  });

  /** …and the page range is what pagination adds ON TOP, not what unlocks the count. */
  it("keeps the count beside the page range when pagination is on", () => {
    render(<ChartTable columns={COLS} rows={mkRows(30)} tableState={{ pagination: { enabled: true, pageSize: 25 } }} />);
    expect(screen.getByText("Showing 30 rows")).toBeInTheDocument();
    expect(screen.getByText("1–25 of 30")).toBeInTheDocument();
  });

  it("renders columns in tableState.columnOrder", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ columnOrder: ["b", "a"] }} />);
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers[0]).toHaveTextContent("b");
    expect(headers[1]).toHaveTextContent("a");
  });

  /**
   * `columnOrder` HOLDS RAW COLUMN NAMES, and `column_settings[name].column_title` renames the
   * HEADER without renaming the column — so the order has to be resolved through `c.name` and
   * never through the text on screen. Resolving through the title drops every renamed column out
   * of the stored order silently: it falls through to the appended tail and lands at the end,
   * which reads as "my drag didn't save".
   *
   * Written because the two-column case cannot see this — with `[b, a]` reversed, losing `a` from
   * the order and re-appending it produces the identical layout, so the mutation survived. Three
   * columns with the renamed one in the MIDDLE of the order is the smallest case that can tell the
   * two apart. This is the seventh instance in this programme of a bug from keying on a display
   * string rather than a raw value, and the second found only by mutating.
   */
  it("resolves columnOrder by raw column name, not by a renamed header", () => {
    const three = [...COLS, { name: "c", type: "varchar" }];
    render(
      <ChartTable
        columns={three}
        rows={[["a0", 0, "c0"]]}
        columnSettings={{ a: { column_title: "Alpha" } }}
        tableState={{ columnOrder: ["c", "a", "b"], pinnedLeft: 2 }}
      />,
    );
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers.map((h) => h.textContent)).toEqual(["c", "Alpha", "b"]);
    // The freeze counts down that same list, so it must not drift with the rename either.
    expect(headers[0].className).toContain("z-30");
    expect(headers[1].className).toContain("z-30");
    expect(headers[2].className).toContain("z-10");
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
