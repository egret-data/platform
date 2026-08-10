import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

/**
 * PR B: for a TABLE there is one column list, not two.
 *
 * The table's `styleColumns` ARE its `resultColumns` (`styleColumnsFor` returns them unfiltered),
 * so the Formatting accordion under the column list was a second row per column carrying the same
 * names in a different order — and the rename two lists away from the visibility toggle. It is now
 * the row's own drill-in. Every other viz keeps the section, because for them `styleColumns` is a
 * genuine subset of the result and so is a different list rather than a duplicate one.
 */
describe("SharedDisplaySections — the table has ONE column list", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "revenue", type: "bigint" }];
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

  it("renders no separate Formatting section for a table", () => {
    render(<SharedDisplaySections vizType="table" {...common} only="columns" />);
    expect(screen.getByText("Columns")).toBeInTheDocument();
    expect(screen.queryByText("Formatting")).not.toBeInTheDocument();
  });

  it("prints each column's name exactly once", () => {
    render(<SharedDisplaySections vizType="table" {...common} only="columns" />);
    expect(screen.getAllByText("revenue")).toHaveLength(1);
  });

  it("puts the formatting behind the row's drill-in instead", () => {
    render(<SharedDisplaySections vizType="table" {...common} only="columns" />);
    expect(screen.getByLabelText("settings revenue")).toBeInTheDocument();
  });

  // A pivot's formattable columns are a SUBSET of its result, so its Formatting list is a
  // different list and stays. Pinned so "drop the section" cannot be over-applied.
  it("keeps the Formatting section for a pivot", () => {
    render(<SharedDisplaySections vizType="pivot_table" {...common}
      styleColumns={[{ name: "revenue", type: "bigint" }]} only="format" />);
    expect(screen.getByText("Formatting")).toBeInTheDocument();
  });
  /**
   * THE TAB THAT WENT EMPTY. Giving the column list its own tab moved the whole section with it,
   * and the table's Display tab rendered nothing at all — `innerHTML: ""`, measured in the browser.
   * 4282 tests were green through that: every one asserts what a tab CONTAINS, and none asserts
   * that Display contains anything.
   *
   * So: the two halves are split by `part`, and both halves are pinned here.
   */
  it("keeps the table's layout settings on Display, not on Columns", () => {
    render(<SharedDisplaySections vizType="table" {...common} only="format" />);
    expect(screen.getByLabelText("Freeze first columns")).toBeInTheDocument();
    expect(screen.getByLabelText("Show row index")).toBeInTheDocument();
    // …and NOT the list, which is the whole point of the split.
    expect(screen.queryByLabelText("Show revenue")).toBeNull();
  });

  it("puts the list on Columns without the layout settings", () => {
    render(<SharedDisplaySections vizType="table" {...common} only="columns" />);
    expect(screen.getByLabelText("Show revenue")).toBeInTheDocument();
    expect(screen.queryByLabelText("Freeze first columns")).toBeNull();
  });

  /** A dashboard card has no tabs to split across, so it takes both halves. */
  it("gives a card both halves when no `only` is passed", () => {
    render(<SharedDisplaySections vizType="table" {...common} />);
    expect(screen.getByLabelText("Show revenue")).toBeInTheDocument();
    expect(screen.getByLabelText("Freeze first columns")).toBeInTheDocument();
  });
});

/**
 * "Display as" — and the two link templates under it — belong to the TABLE and to nothing else.
 *
 * `ChartTable.renderTableCell` is the only renderer in the app that branches on `view_as`; every
 * other viz turns a cell value into text through `formatValue`, which does not consult it (and
 * says so, exhaustively, in `format.ts`'s `CONSULTED_BY_FORMAT_DATE`). So a pivot row-dimension, a
 * cartesian x-axis, a pie dimension, a sankey node column and an object-detail field all offered a
 * "Display as → Link" select and two template inputs that saved and painted nothing.
 *
 * VERIFIED BY RUNNING, not only by reading: a probe rendered all nineteen viz types through
 * `ChartRenderer` with `view_as`/`link_text`/`link_url` behind recording getters. `table` was the
 * only type whose stacks reached a consuming read (`ChartTable.tsx` `renderTableCell`); every other
 * touch was an object SPREAD into a `ColumnFormat` bound for `formatValue`
 * (`cartesian.ts` `{...col, compact}`, `scalar.ts` `compactifyValue`, `boxplot.ts` `fmt`) or a
 * `JSON.stringify` memo key. `viewAsIsTableOnly.test.tsx` keeps that conclusion pinned.
 *
 * The gate is `allowMiniBar`'s, reused rather than re-invented — `show_mini_bar` had exactly this
 * defect and was closed the same way.
 */
describe("SharedDisplaySections — Display as is the table's, and only the table's", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "revenue", type: "bigint" }];
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

  // Every viz that mounts the shared Formatting accordion, i.e. all of them but the table. Named
  // one by one rather than looped over `VizType` so that a new viz is a deliberate addition here
  // — but broad enough that the gate cannot be wired for the pivot alone and left off the rest.
  const NON_TABLE: VizType[] = ["pivot_table", "bar", "line", "pie", "sankey", "object", "map"];

  it.each(NON_TABLE)("%s offers no Display as", async (vizType) => {
    render(<SharedDisplaySections vizType={vizType} {...common} only="format" />);
    // The accordion body only mounts once its column row is expanded.
    await userEvent.click(screen.getByText("region"));
    // Vacuity guard. A row that failed to expand renders no fields at all, and every assertion
    // below would then pass while proving nothing — which is the exact shape of the mutation this
    // whole PR's Group C is about. `Column title` is in the body for a column of any type.
    expect(screen.getByText("Column title")).toBeInTheDocument();
    expect(screen.queryByLabelText("Display as")).not.toBeInTheDocument();
    expect(screen.queryByText("Link text")).not.toBeInTheDocument();
    expect(screen.queryByText("Link URL")).not.toBeInTheDocument();
  });

  // The other half of the same rule: where the setting IS read, the control has to still be there.
  // Without this, "delete the control" passes the assertion above just as well as "gate it".
  it("the table still offers it, in the row's drill-in", async () => {
    render(<SharedDisplaySections vizType="table" {...common} only="columns" />);
    await userEvent.click(screen.getByLabelText("settings region"));
    expect(screen.getByLabelText("Display as")).toBeInTheDocument();
  });
});

/**
 * #410 moved the table's formatting into a per-row drill-in, so `ColumnFormatFields` now has TWO
 * mounts — the accordion every other viz gets, and `ColumnSettingsPopover`. A control added to the
 * temporal branch has to arrive at both, and the only reason it does is that they share the
 * component rather than each holding a copy. Pinned here so a future "just add it to the accordion"
 * cannot quietly re-fork them.
 */
describe("SharedDisplaySections — the date/time controls reach both surfaces", () => {
  const cols = [{ name: "ordered_at", type: "timestamp" }];
  const common = {
    resultColumns: cols,
    styleColumns: cols,
    tableState: {},
    columnSettings: { ordered_at: { time_enabled: "minutes" as const } },
    conditionalFormatting: [],
    onTableState: vi.fn(),
    onColumnSettings: vi.fn(),
    onConditionalFormatting: vi.fn(),
  };

  it("in the pivot's Formatting accordion", async () => {
    render(<SharedDisplaySections vizType="pivot_table" {...common} only="format" />);
    await userEvent.click(screen.getByText("ordered_at"));
    expect(screen.getByLabelText("Show the time")).toBeInTheDocument();
    expect(screen.getByLabelText("Time style")).toBeInTheDocument();
  });

  it("and in the table's row drill-in", async () => {
    render(<SharedDisplaySections vizType="table" {...common} only="columns" />);
    await userEvent.click(screen.getByLabelText("settings ordered_at"));
    expect(screen.getByLabelText("Show the time")).toBeInTheDocument();
    expect(screen.getByLabelText("Time style")).toBeInTheDocument();
  });
});

/**
 * THE MAP IS THE ONE THAT NARROWS RATHER THAN THREADS.
 *
 * Four viz types fell through `styleColumnsFor`'s bare `return resultColumns` while their builders
 * read `columnSettings` for the metric only — funnel, box plot, sankey and map. Three of them are
 * fixed by THREADING the dimension's format into the label the builder draws, because that is what
 * Metabase does (`FunnelNormal`'s `formatDimension`, `echarts/boxplot/model`'s
 * `createXAxisFormatter`, `sankey/model/formatters`' `node`). The map is the exception on BOTH
 * counts and so takes the other fix.
 *
 * WHAT METABASE DOES: `ChoroplethMap.tsx`'s only `settings.column?.(…)` call is on the METRIC, for
 * the legend. The region cell is run through `getCanonicalRowKey`, which lowercases and maps names
 * to codes, and the name actually PAINTED comes off the geojson feature's own `region_name`
 * property — never the column. `Map.tsx` passes `showColumnSetting` on neither its dimension nor
 * its metric, so its settings sidebar exposes no per-column format widget at all.
 *
 * WHY EGRET CANNOT THREAD IT EVEN IF IT WANTED TO: `MapChart` builds every series datum as
 * `{ name: normalizeKey(cell) }`, and ECharts matches that string against the registered geojson's
 * `properties.key` to decide which shape to shade. A prefix or a date style applied there does not
 * restyle a label — it breaks the match and the region stops being drawn. There is no second place
 * the raw region text is rendered: the tooltip's name is the matched key.
 *
 * The METRIC stays, and that is a deliberate divergence from Metabase's "no widget at all". egret's
 * map genuinely formats its metric — `MapChart`'s `fmt` feeds both `itemTooltip` and the
 * `visualMap` range labels, and `options/map.ts` records that being added on purpose — so removing
 * the control would take away a setting that IS honoured, which is the same defect in the mirror.
 */
describe("styleColumnsFor — map", () => {
  const cols = [
    { name: "state", type: "varchar" },
    { name: "revenue", type: "double" },
  ];

  it("offers the metric only — the region is a geojson key, not a label", () => {
    const params = { map: { region: "state", metric: "revenue" } } as never;
    expect(styleColumnsFor("map", params, cols).map((c) => c.name)).toEqual(["revenue"]);
  });

  it("offers nothing while the map is still unconfigured", () => {
    expect(styleColumnsFor("map", {} as never, cols)).toEqual([]);
  });

  // The three that thread keep their dimension, and this is what stops a later narrowing from
  // being copied across the fall-through by symmetry: they are NOT the same case.
  it.each(["funnel", "boxplot", "sankey"] as const)("still offers every column on a %s", (viz) => {
    expect(styleColumnsFor(viz, {} as never, cols).map((c) => c.name)).toEqual(["state", "revenue"]);
  });

});
