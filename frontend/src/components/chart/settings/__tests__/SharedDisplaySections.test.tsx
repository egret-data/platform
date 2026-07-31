import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { SharedDisplaySections, styleColumnsFor, ruleColumnsFor } from "../SharedDisplaySections";
import type { VizType } from "@/api/charts";

// Renders with no result columns yet and returns the empty-state paragraph's text. Every prop
// besides vizType is a no-op stand-in since the empty-state branch returns before any of the
// real sections mount.
function emptyStateText(vizType: VizType): string {
  const { container, unmount } = render(
    <SharedDisplaySections
      vizType={vizType}
      resultColumns={[]}
      styleColumns={[]}
      tableState={{}}
      columnSettings={{}}
      conditionalFormatting={[]}
      onTableState={() => {}}
      onColumnSettings={() => {}}
      onConditionalFormatting={() => {}}
    />,
  );
  const text = container.querySelector("p")?.textContent ?? "";
  unmount();
  return text;
}

describe("SharedDisplaySections empty state", () => {
  it("names columns and running for a table with no result columns", () => {
    const text = emptyStateText("table");
    expect(text).toMatch(/column/i);
    expect(text).toMatch(/run/i);
  });

  // Load-bearing: a single shared sentence (the old "Run the chart to configure column
  // formatting.") would satisfy the table assertion above just as well — it mentions both
  // "column" and "run". That alone can't prove the message was tailored per viz. Comparing the
  // table's text against a cartesian viz's text forces them to actually diverge; a shared
  // sentence fails this test even though it would pass the first one.
  it("shows different empty-state wording for a cartesian viz than for a table", () => {
    const tableText = emptyStateText("table");
    const barText = emptyStateText("bar");
    expect(barText).not.toEqual(tableText);
  });

  it("names what a pivot table needs when it has no result columns", () => {
    const text = emptyStateText("pivot_table");
    expect(text).toMatch(/pivot|rows|columns/i);
  });
});

describe("SharedDisplaySections — split into halves", () => {
  const cols = [{ name: "rev", type: "bigint" }];
  const common = {
    resultColumns: cols,
    styleColumns: cols,
    tableState: {},
    columnSettings: {},
    conditionalFormatting: [],
    onTableState: vi.fn(),
    onColumnSettings: vi.fn(),
    onConditionalFormatting: vi.fn(),
  };

  it("renders both halves by default, so existing callers are unchanged", () => {
    render(<SharedDisplaySections vizType="pivot_table" {...common} />);
    expect(screen.getByText("Formatting")).toBeInTheDocument();
    expect(screen.getByText("Conditional formatting")).toBeInTheDocument();
  });

  it("only=format leaves the rules out", () => {
    render(<SharedDisplaySections vizType="pivot_table" {...common} only="format" />);
    expect(screen.getByText("Formatting")).toBeInTheDocument();
    expect(screen.queryByText("Conditional formatting")).not.toBeInTheDocument();
  });

  it("only=conditional leaves the formatting out", () => {
    render(<SharedDisplaySections vizType="pivot_table" {...common} only="conditional" />);
    expect(screen.queryByText("Formatting")).not.toBeInTheDocument();
    expect(screen.getByText("Conditional formatting")).toBeInTheDocument();
  });
});

describe("ruleColumnsFor", () => {
  const cols = [
    { name: "region", type: "varchar" },
    { name: "order_date", type: "date" },
    { name: "revenue", type: "bigint" },
  ];
  const params = {
    rows: ["region"], cols: ["order_date"], metrics: ["revenue"],
  } as unknown as Parameters<typeof ruleColumnsFor>[1];

  it("a pivot may only colour its measures", () => {
    // `buildConditional` is keyed by metric, so a rule on a dimension would save, read back, and
    // colour nothing — a setting that cannot take effect. Widening `styleColumnsFor` to include
    // dimensions (so they could be FORMATTED) briefly widened this too; it must not.
    expect(ruleColumnsFor("pivot_table", params, cols).map((c) => c.name)).toEqual(["revenue"]);
  });

  it("but a pivot may FORMAT its dimensions", () => {
    expect(styleColumnsFor("pivot_table", params, cols).map((c) => c.name))
      .toEqual(["region", "order_date", "revenue"]);
  });

  it("a table may colour every column", () => {
    expect(ruleColumnsFor("table", params, cols).map((c) => c.name))
      .toEqual(["region", "order_date", "revenue"]);
  });
});

describe("styleColumnsFor — pie", () => {
  const cols = [
    { name: "region", type: "varchar" },
    { name: "order_date", type: "date" },
    { name: "revenue", type: "double" },
  ];

  // The pie returned [] unconditionally, so its Formatting section never appeared — while the
  // renderer had always resolved `columnSettings[metric]` for every label, tooltip and the centre
  // total, and the slice names resolve their dimension's format the same way.
  it("offers the pie's dimensions and its metric", () => {
    const params = { pie: { dimensions: ["order_date", "region"], metric: "revenue" } } as never;
    expect(styleColumnsFor("pie", params, cols).map((c) => c.name))
      .toEqual(["region", "order_date", "revenue"]);
  });

  it("offers nothing while the pie is still unconfigured", () => {
    expect(styleColumnsFor("pie", {} as never, cols)).toEqual([]);
  });
});
