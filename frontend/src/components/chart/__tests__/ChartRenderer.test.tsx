// src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { useState } from "react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams, PieConfig, VizType } from "@/api/charts";

// Captures the `option` prop each render passes to EChartsRenderer, so tests can assert on
// what actually reaches the renderer — the exact seam where four settings (maxCategories,
// compactValues, seriesOrder, hiddenSeries) were once silently dropped by a hand-maintained
// field whitelist in ChartRenderer.
let mockLastOption: unknown;
// Configurable so a test can simulate a click landing on a specific ring/series; defaults match
// what every pre-existing test here expects.
type MockClick = {
  name?: string;
  seriesName?: string;
  value?: unknown;
  /** Root-to-node path, as a sunburst click carries it — entry 0 is ECharts' virtual root. */
  treePathInfo?: { name?: string }[];
  /** The datum the element was built from. The pie's drill reads its `key`. */
  data?: unknown;
  /** Which series the clicked element belongs to. The cartesian drill reads `__raw` through it. */
  seriesIndex?: number;
  /** The datum's index within its series. The box plot's drill reads the category from it. */
  dataIndex?: number;
  /** Which kind of element was clicked — the sankey's node vs edge. */
  dataType?: string;
  x: number;
  y: number;
};
let mockClickPayload: MockClick = { name: "EU", seriesName: "rev", value: 10, x: 3, y: 4 };
/** The hover the mocked chart reports when a test "points at" it. */
let mockHoverPayload: { name?: string; value?: unknown; treePathInfo?: { name?: string }[] } | null = null;
/** The `overlay` prop each render passes — the merge-mode partial option the pie's centre readout
 * rides on, and the seam a hover has to land on WITHOUT disturbing `option`. */
let mockLastOverlay: unknown;
/** The `highlightDataIndex` prop each render passes — the legend's half of the two-way hover. */
let mockLastHighlight: number | undefined;

vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option, overlay, highlightDataIndex, onPointClick, onPointHover }: {
    option: unknown;
    overlay?: unknown;
    highlightDataIndex?: number;
    onPointClick?: (p: MockClick) => void;
    onPointHover?: (p: unknown) => void;
  }) => {
    mockLastOption = option;
    mockLastOverlay = overlay;
    mockLastHighlight = highlightDataIndex;
    return (
      <>
        <button data-testid="echarts" onClick={() => onPointClick?.(mockClickPayload)} />
        <button data-testid="echarts-hover" onClick={() => onPointHover?.(mockHoverPayload)} />
      </>
    );
  },
}));

const params = (p: Partial<ChartParams> = {}): ChartParams => ({
  spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
  ...p,
});

describe("ChartRenderer", () => {
  beforeEach(() => {
    mockLastOption = undefined;
    mockLastOverlay = undefined;
    mockLastHighlight = undefined;
    mockHoverPayload = null;
    mockClickPayload = { name: "EU", seriesName: "rev", value: 10, x: 3, y: 4 };
  });

  it("renders a table viz with formatted cells", () => {
    render(
      <ChartRenderer
        vizType="table"
        params={params()}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1234.5]]}
      />,
    );
    expect(screen.getByText("region")).toBeInTheDocument();
    expect(screen.getByText("1,234.5")).toBeInTheDocument();
  });

  it("renders a big number for scalar viz", () => {
    render(
      <ChartRenderer
        vizType="scalar"
        params={params({ scalar: { metric: "revenue" } })}
        columns={[{ name: "revenue", type: "double" }]}
        rows={[[42000]]}
      />,
    );
    expect(screen.getByText((t) => t.replace(/,/g, "") === "42000")).toBeInTheDocument();
  });

  // THE SEAM, not the component. Every assertion below would still pass if `BigNumber` were
  // perfect and this branch dropped the setting on the floor — which is exactly what happened to
  // pie (decimals, sliceNames, hiddenSlices, sliceOrder) and to cartesian before it, because the
  // component tests called the builder directly and never crossed this line. The scalar branch
  // spreads its config; these tests are what would notice if someone un-spread it.
  describe("scalar settings reach the chart through ChartRenderer", () => {
    const cols = [{ name: "revenue", type: "double" }];

    it("forwards conditional-colour segments", () => {
      render(
        <ChartRenderer
          vizType="scalar"
          params={params({
            scalar: {
              metric: "revenue",
              segments: [{ min: 0, max: 100, color: "#e75454", label: "Low" }],
            },
          })}
          columns={cols}
          rows={[[42]]}
        />,
      );
      expect(screen.getByText("42")).toHaveStyle({ color: "#e75454" });
      expect(screen.getByText("Low")).toBeInTheDocument();
    });

    it("forwards label and sublabel", () => {
      render(
        <ChartRenderer
          vizType="scalar"
          params={params({ scalar: { metric: "revenue", label: "Revenue", sublabel: "this quarter" } })}
          columns={cols}
          rows={[[42]]}
        />,
      );
      expect(screen.getByText("Revenue")).toBeInTheDocument();
      expect(screen.getByText("this quarter")).toBeInTheDocument();
    });

    it("forwards the drill handler", () => {
      const onValueClick = vi.fn();
      render(
        <ChartRenderer
          vizType="scalar"
          params={params({ scalar: { metric: "revenue" } })}
          columns={cols}
          rows={[[42]]}
          onValueClick={onValueClick}
        />,
      );
      fireEvent.click(screen.getByRole("button"));
      expect(onValueClick).toHaveBeenCalledWith({ dims: [] }, expect.any(Number), expect.any(Number));
    });
  });

  it("dispatches cartesian viz to the ECharts renderer", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["revenue"] } })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 10], ["US", 20]]}
      />,
    );
    expect(screen.getByTestId("echarts")).toBeInTheDocument();
  });

  it("shows an empty state for cartesian with no rows", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["revenue"] } })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[]}
      />,
    );
    expect(screen.getByText("No data")).toBeInTheDocument();
  });

  it("shows an empty state for gauge with no rows", () => {
    render(
      <ChartRenderer
        vizType="gauge"
        params={params({ gauge: { metric: "revenue" } })}
        columns={[{ name: "revenue", type: "double" }]}
        rows={[]}
      />,
    );
    expect(screen.getByText("No data")).toBeInTheDocument();
  });

  it("dispatches gauge with rows to ECharts renderer", () => {
    render(
      <ChartRenderer
        vizType="gauge"
        params={params({ gauge: { metric: "revenue" } })}
        columns={[{ name: "revenue", type: "double" }]}
        rows={[[75]]}
      />,
    );
    expect(screen.getByTestId("echarts")).toBeInTheDocument();
  });

  it("renders progress viz with a value", () => {
    render(
      <ChartRenderer
        vizType="progress"
        params={params({ progress: { metric: "revenue", goal: 100 } })}
        columns={[{ name: "revenue", type: "double" }]}
        rows={[[60]]}
      />,
    );
    // ProgressChart renders data-progress-value with the formatted metric
    const valueEl = document.querySelector("[data-progress-value]");
    expect(valueEl).not.toBeNull();
    expect(valueEl!.textContent).toBe("60");
  });

  it("renders pivot_table without throwing and shows dimension header", () => {
    render(
      <ChartRenderer
        vizType="pivot_table"
        params={params({
          rows: ["region"],
          cols: [],
          metrics: ["revenue"],
          showRowTotals: false,
          showColTotals: false,
        })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1000], ["US", 2000]]}
      />,
    );
    // PivotTable renders rowDim names as column headers
    expect(screen.getByText("region")).toBeInTheDocument();
  });

  it("calls onValueClick with the xAxis dim when a cartesian point is clicked", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["rev"] } })}
        columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]}
        rows={[["EU", 10]]}
        onValueClick={onValueClick}
      />,
    );
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "region", value: "EU" }] }, 3, 4);
  });

  // A pie click reports the slice's display NAME, which on a nested chart names only the innermost
  // ring. The drill reads the datum's aggregation key instead — the whole root-to-here path in raw
  // values — so an outer slice filters by its ancestors too. `␟` is options/pie.ts' KEY_SEP.
  it("drills a nested pie slice on its whole ancestor path, not just its own ring", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="pie"
        params={params({ pie: { dimensions: ["region", "category", "channel"], metric: "rev" } })}
        columns={[
          { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
          { name: "channel", type: "varchar" }, { name: "rev", type: "double" },
        ]}
        rows={[["EU", "Widgets", "Direct", 10]]}
        onValueClick={onValueClick}
      />,
    );

    mockClickPayload = { name: "EU", value: 40, data: { key: "EU" }, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "region", value: "EU" }] }, 1, 2);

    // Ring 1 — "Widgets" under "EU" must NOT drill to every region's Widgets.
    mockClickPayload = { name: "Widgets", value: 10, data: { key: "EU␟Widgets" }, x: 5, y: 6 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith(
      { dims: [{ col: "region", value: "EU" }, { col: "category", value: "Widgets" }] }, 5, 6,
    );

    // Ring 2.
    mockClickPayload = { name: "Direct", value: 10, data: { key: "EU␟Widgets␟Direct" }, x: 7, y: 8 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({
      dims: [
        { col: "region", value: "EU" },
        { col: "category", value: "Widgets" },
        { col: "channel", value: "Direct" },
      ],
    }, 7, 8);
  });

  // "Other" is a bucket the chart invented, not a value in the column. Drilling it sent
  // `region = 'Other'`, which matches no row — an empty dialog reported as the slice's contents.
  it("refuses to drill an Other bucket rather than filtering by a value that does not exist", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="pie"
        params={params({ pie: { dimensions: ["region"], metric: "rev" } })}
        columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]}
        rows={[["EU", 10]]}
        onValueClick={onValueClick}
      />,
    );
    mockClickPayload = { name: "Other", value: 1, data: { key: "\u0000__other__\u0000" }, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).not.toHaveBeenCalled();

    // …and a datum with no key at all (nothing else in the app sends one) is inert, not a throw.
    mockClickPayload = { name: "EU", value: 40, x: 9, y: 10 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).not.toHaveBeenCalled();
  });

  // Fourth chart family with this defect: a slice standing for a whole month drilled to the single
  // day its label happens to name.
  it("attaches the temporal unit so a bucketed slice drills to its period", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="pie"
        params={params({
          pie: { dimensions: ["order_date"], metric: "rev" },
          spec: {
            metrics: ["rev"], dimensions: ["order_date"], columns: [], filters: [], orderby: [],
            row_limit: 1000, rollup_groups: [], temporal_units: { order_date: "month" },
          },
        })}
        columns={[{ name: "order_date", type: "date" }, { name: "rev", type: "double" }]}
        rows={[["2026-01-01", 10]]}
        onValueClick={onValueClick}
      />,
    );
    mockClickPayload = { name: "2026-01-01", value: 10, data: { key: "2026-01-01" }, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith(
      { dims: [{ col: "order_date", value: "2026-01-01", unit: "month" }] }, 1, 2,
    );
  });

  it("wires onCellDrill on a table card when onValueClick is set", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="table"
        params={params({ spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } })}
        columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]}
        rows={[["EU", 10]]}
        onValueClick={onValueClick}
      />,
    );
    // a dimension cell is now clickable (cursor-pointer); clicking it fires onValueClick
    const cell = screen.getByText("EU").closest("td")!;
    expect(cell.className).toContain("cursor-pointer");
    fireEvent.click(cell);
    expect(onValueClick).toHaveBeenCalled();
  });

  // These two pin the ChartRenderer -> buildCartesianOption seam itself: every field on
  // CartesianConfig must reach the option, not just the ones someone remembered to list.
  //
  // Cartesian's `option` prop is a FUNCTION of the measured container size, as pie's already was,
  // so both read it by CALLING it with a size — see `pieOption` below, which does the same.
  const cartesianOption = <T,>(): T =>
    (mockLastOption as (s: { width: number; height: number }) => T)({ width: 640, height: 400 });

  it("forwards cartesian.maxCategories through to the built option", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["revenue"], maxCategories: 3 } })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["a", 10], ["b", 8], ["c", 6], ["d", 4], ["e", 2]]}
      />,
    );
    const data = cartesianOption<{ xAxis: { data: string[] } }>().xAxis.data;
    expect(data[data.length - 1]).toBe("Other");
  });

  it("forwards cartesian.hiddenSeries through to the built option", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({
          cartesian: { xAxis: "region", series: "cat", yMetrics: ["revenue"], hiddenSeries: ["B"] },
        })}
        columns={[
          { name: "region", type: "varchar" },
          { name: "cat", type: "varchar" },
          { name: "revenue", type: "double" },
        ]}
        rows={[["a", "A", 10], ["a", "B", 5], ["b", "A", 20], ["b", "B", 15]]}
      />,
    );
    const names = cartesianOption<{ series: { name?: string }[] }>().series.map((s) => s.name);
    expect(names).not.toContain("B");
    expect(names).toContain("A");
  });

  // The pie branch has the same seam, and had the same defect: a hand-listed field whitelist in
  // ChartRenderer meant decimals/sliceNames/hiddenSlices/sliceOrder never reached buildPieOption,
  // so slice rename, hide, reorder and percent precision were unreachable from the app entirely.
  // pie.test.ts missed it because it calls buildPieOption directly, bypassing this exact line.
  // One test per field, so dropping any single one of them fails on its own.
  describe("-> buildPieOption seam", () => {
    const pieCols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const pieRows = [["A", 10], ["B", 30], ["C", 5]];
    // Each DATUM carries the label formatter that actually renders; the series-level label has
    // none (it could never run — a datum's own always wins), so reading the built label text means
    // reading it per slice.
    type BuiltPie = {
      series: { data: { name: string; label: { formatter: () => string } }[] }[];
    };
    const pieOption = (pie: Partial<PieConfig>): BuiltPie => {
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions: ["d"], metric: "m", ...pie } })}
          columns={pieCols}
          rows={pieRows}
        />,
      );
      // Pie's `option` prop is a function of the measured container size, so it is read by CALLING
      // it — the same way `cartesianOption` above reads the cartesian family's.
      return (mockLastOption as (s: { width: number; height: number }) => BuiltPie)({ width: 400, height: 400 });
    };

    it("forwards pie.sliceNames to the built option", () => {
      const data = pieOption({ sliceNames: { A: "Alpha" } }).series[0].data;
      expect(data.map((d) => d.name)).toContain("Alpha");
    });

    it("forwards pie.hiddenSlices to the built option", () => {
      const data = pieOption({ hiddenSlices: ["B"] }).series[0].data;
      expect(data.map((d) => d.name)).not.toContain("B");
    });

    it("forwards pie.sliceOrder to the built option", () => {
      // Natural (value-desc) order is B, A, C — sliceOrder must pull A to the front.
      const data = pieOption({ sliceOrder: ["A"] }).series[0].data;
      expect(data.map((d) => d.name)[0]).toBe("A");
    });

    it("forwards pie.decimals to the built option", () => {
      // A is 10 of this fixture's 45, i.e. 22.22...% — so an explicit precision renders visibly
      // differently from the unset one, which the pie now computes from the data itself (at two
      // significant digits on a slice label, this chart's shares want no decimals at all).
      const labelOfA = (pie: Partial<PieConfig>) =>
        pieOption(pie).series[0].data.find((d) => d.name === "A")!.label.formatter();
      expect(labelOfA({ percentVisibility: "inside", showLabels: false })).toBe("22%");
      expect(labelOfA({ percentVisibility: "inside", showLabels: false, decimals: 3 })).toBe("22.222%");
    });
  });

  // A pie whose every value is zero has ROWS — so the `rows.length === 0` guard every viz shares
  // does not fire — but nothing it can draw: the zero/negative filter empties ring 0, and what
  // reached the canvas was an empty sunburst, i.e. a blank rectangle with no message on it. The
  // aria description said "No data." all along, so the chart disagreed with its own text
  // alternative.
  describe("a pie with nothing to draw", () => {
    const pieCols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const renderPie = (rows: unknown[][]) =>
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions: ["d"], metric: "m" } })}
          columns={pieCols}
          rows={rows}
        />,
      );

    it("says so instead of drawing an empty canvas", () => {
      renderPie([["A", 0], ["B", 0], ["C", 0]]);
      expect(screen.getByText("No values to chart")).toBeInTheDocument();
      expect(screen.queryByTestId("echarts")).not.toBeInTheDocument();
    });

    it("says the same for values it cannot chart at all", () => {
      renderPie([["A", null], ["B", "n/a"]]);
      expect(screen.getByText("No values to chart")).toBeInTheDocument();
    });

    it("still draws a pie that has one slice left", () => {
      renderPie([["A", 0], ["B", 7]]);
      expect(screen.queryByText("No values to chart")).not.toBeInTheDocument();
      expect(screen.getByTestId("echarts")).toBeInTheDocument();
    });

    // "No data" is what an empty RESULT says, and this result is not empty — the rows came back,
    // they just have nothing a pie can show a share of.
    it("does not claim the query returned nothing", () => {
      renderPie([["A", 0]]);
      expect(screen.queryByText("No data")).not.toBeInTheDocument();
    });
  });

  // `PieBreakdown` shipped built and unit-tested but mounted NOWHERE — the same shape as the four
  // pie settings that were implemented, panelled and unreachable because ChartRenderer never
  // forwarded them. A component's own tests can never catch that; only a test at this seam can.
  describe("a nested pie's structured alternative reaches the page", () => {
    const cols = [
      { name: "r", type: "varchar" }, { name: "p", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows = [["US", "Widget", 30], ["US", "Gadget", 10], ["EU", "Widget", 60]];

    const renderPie = (dimensions: string[]) =>
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions, metric: "m" } })}
          columns={cols}
          rows={rows}
        />,
      );

    it("mounts the table for a multi-ring pie", () => {
      renderPie(["r", "p"]);
      expect(screen.getByRole("table")).toBeInTheDocument();
    });

    it("carries every slice, outer rings included", () => {
      renderPie(["r", "p"]);
      const table = screen.getByRole("table");
      // Ring 0 and ring 1 both present — the ring-1 names are exactly what ariaDescription omits.
      ["US", "EU", "Widget", "Gadget"].forEach((name) =>
        expect(within(table).getAllByText(name).length).toBeGreaterThan(0));
    });

    it("stays out of the way of a single-ring pie, which aria already reads in full", () => {
      renderPie(["r"]);
      expect(screen.queryByRole("table")).not.toBeInTheDocument();
    });
  });

  // The pie's legend is React markup beside the canvas now, not an ECharts `legend` component
  // inside it — so ChartRenderer is where the two halves are put together, and this is the seam
  // that decides which layout they are put together in.
  describe("-> PieLegend seam", () => {
    const pieCols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const pieRows = [["A", 10], ["B", 30]];
    const renderPie = (pie: Partial<PieConfig> = {}) =>
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions: ["d"], metric: "m", ...pie } })}
          columns={pieCols}
          rows={pieRows}
        />,
      );
    /** Makes every element measure as this size, which is what the placement decision reads. */
    const measuringAs = (width: number, height: number) =>
      vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
        () => ({ width, height, top: 0, left: 0, right: width, bottom: height, x: 0, y: 0, toJSON() {} }) as DOMRect,
      );

    afterEach(() => vi.restoreAllMocks());

    it("renders the ring-0 slices as markup, with no legend left in the option", () => {
      renderPie();
      expect(screen.getByRole("button", { name: /^B/ })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /^A/ })).toBeInTheDocument();
      const built = (mockLastOption as (s: { width: number; height: number }) => object)({ width: 400, height: 400 });
      expect(built).not.toHaveProperty("legend");
    });

    it("draws no legend at all when the chart turns it off", () => {
      const { container } = renderPie({ showLegend: false });
      expect(container.querySelector("[data-pie-legend]")).toBeNull();
    });

    // The real measured host from the Metabase comparison, and one shaped the opposite way.
    it("puts the legend beside a wide, short host and below a narrow, tall one", () => {
      measuringAs(830, 661);
      const { container: wide } = renderPie();
      expect(wide.querySelector("[data-pie-legend]")).toHaveAttribute("data-pie-legend", "side");

      vi.restoreAllMocks();
      measuringAs(360, 720);
      const { container: narrow } = renderPie();
      expect(narrow.querySelector("[data-pie-legend]")).toHaveAttribute("data-pie-legend", "bottom");
    });

    it("tells the option builder which layout it is in, so the circle fills the right box", () => {
      measuringAs(830, 661);
      renderPie();
      const built = (mockLastOption as (s: { width: number; height: number }) => {
        series: { radius: number[] }[];
      })({ width: 640, height: 661 });
      // The side layout's 0.85 of min(640, 661) = a 544px circle, i.e. r = 272 — where the bottom
      // layout's 0.70 would only have drawn 448.
      expect(built.series[0].radius[1]).toBeCloseTo(272, 6);
    });

    // The ECharts legend this replaces toggled its slice off on click; that has to keep working.
    it("drops a slice from the option when its legend entry is clicked, and puts it back", () => {
      renderPie();
      const built = () =>
        (mockLastOption as (s: { width: number; height: number }) => {
          series: { data: { name: string }[] }[];
        })({ width: 400, height: 400 }).series[0].data.map((d) => d.name);
      expect(built()).toEqual(["B", "A"]);

      fireEvent.click(screen.getByRole("button", { name: /^B/ }));
      expect(built()).toEqual(["A"]);
      expect(screen.getByRole("button", { name: /^B/ })).toHaveAttribute("aria-pressed", "false");

      fireEvent.click(screen.getByRole("button", { name: /^B/ }));
      expect(built()).toEqual(["B", "A"]);
    });
  });

  // Metabase's hole says what you are pointing at; egret's said "Total" and nothing else. The
  // interesting half of this is WHERE the hover lands: not on `option`, whose rebuild runs a
  // per-slice measureText pass over every ring and whose re-application would tear the sunburst
  // down mid-hover, but on a merge-mode `graphic` overlay that touches only the hole's own group.
  describe("-> the hole reads out the slice under the pointer", () => {
    const cols2 = [
      { name: "d", type: "varchar" }, { name: "c", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["A", "x", 10], ["B", "y", 30]];
    const renderPie = () =>
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions: ["d", "c"], metric: "m" } })}
          columns={cols2}
          rows={rows2}
        />,
      );
    const holeText = () =>
      (mockLastOverlay as (s: { width: number; height: number }) => {
        graphic: { children: { style: { text: string } }[] }[];
      })({ width: 400, height: 400 }).graphic[0].children.map((g) => g.style.text);
    const hover = (payload: typeof mockHoverPayload) => {
      mockHoverPayload = payload;
      fireEvent.click(screen.getByTestId("echarts-hover"));
    };

    it("shows the chart total until something is pointed at", () => {
      renderPie();
      expect(holeText()).toEqual(["40", "Total"]);
    });

    it("swaps to the pointed-at slice, and back again when the pointer leaves", () => {
      renderPie();
      hover({ name: "B", value: 30, treePathInfo: [{ name: "" }, { name: "B" }] });
      expect(holeText()).toEqual(["30", "B"]);

      hover({ name: "y", value: 30, treePathInfo: [{ name: "" }, { name: "B" }, { name: "y" }] });
      expect(holeText()).toEqual(["30", "y"]);

      hover(null);
      expect(holeText()).toEqual(["40", "Total"]);
    });

    // THE POINT OF THE OVERLAY. A hover that changed `option` would re-run the fit pass and, worse,
    // make EChartsRenderer re-apply the whole thing with notMerge — throwing away the emphasis
    // state the hover itself just produced.
    it("leaves the expensive option untouched while the pointer moves", () => {
      renderPie();
      const before = mockLastOption;
      hover({ name: "B", value: 30, treePathInfo: [{ name: "" }, { name: "B" }] });
      expect(mockLastOption).toBe(before);
      hover(null);
      expect(mockLastOption).toBe(before);
    });

    // A pie with no hole has nothing to read out into; hovering must not conjure text into the
    // middle of a solid disc.
    it("sends no overlay at all for a pie with no hole", () => {
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions: ["d"], metric: "m" } })}
          columns={cols2}
          rows={rows2}
        />,
      );
      expect(mockLastOverlay).toBeUndefined();
    });
  });

  // Metabase's legend and chart follow each other: hovering a slice greys every legend entry but
  // its ring-0 ancestor's, and hovering a legend entry highlights that slice AND swaps the hole.
  // egret's legend is React markup, so one direction is component state; the other is a
  // `dispatchAction`, which is why the entries carry a datum index at all.
  describe("-> the legend and the chart follow each other's hover", () => {
    const cols2 = [
      { name: "d", type: "varchar" }, { name: "c", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["A", "x", 10], ["B", "y", 30]];
    const renderPie = () =>
      render(
        <ChartRenderer
          vizType="pie"
          params={params({ pie: { dimensions: ["d", "c"], metric: "m" } })}
          columns={cols2}
          rows={rows2}
        />,
      );
    const holeText = () =>
      (mockLastOverlay as (s: { width: number; height: number }) => {
        graphic: { children: { style: { text: string } }[] }[];
      })({ width: 400, height: 400 }).graphic[0].children.map((g) => g.style.text);
    const faded = () =>
      Array.from(document.querySelectorAll("[data-pie-legend] button"))
        .filter((b) => b.hasAttribute("data-pie-legend-faded"))
        .map((b) => b.querySelector("[title]")?.textContent);

    it("greys every legend entry but the hovered slice's ring-0 ancestor", () => {
      renderPie();
      expect(faded()).toEqual([]);
      // An OUTER-ring slice: the entry that stays lit is its ancestor's, not its own — the outer
      // ring has no legend entry at all.
      mockHoverPayload = { name: "y", value: 30, treePathInfo: [{ name: "" }, { name: "B" }, { name: "y" }] };
      fireEvent.click(screen.getByTestId("echarts-hover"));
      expect(faded()).toEqual(["A"]);

      mockHoverPayload = null;
      fireEvent.click(screen.getByTestId("echarts-hover"));
      expect(faded()).toEqual([]);
    });

    it("highlights the slice and reads it out when a legend entry is hovered", () => {
      renderPie();
      fireEvent.mouseEnter(screen.getByRole("button", { name: /^A/ }));
      // "A" is the second ring-0 slice (B, 30 sorts first and takes its own child with it), so its
      // datum is index 3 in the sunburst's flattened tree: root, B, y, A.
      expect(mockLastHighlight).toBe(3);
      expect(holeText()).toEqual(["10", "A"]);
      expect(faded()).toEqual(["B"]);

      fireEvent.mouseLeave(screen.getByRole("button", { name: /^A/ }));
      expect(mockLastHighlight).toBeUndefined();
      expect(holeText()).toEqual(["40", "Total"]);
    });

    // ECharts already runs its own emphasis/blur for whatever the pointer is on inside the canvas;
    // dispatching a second one would fight it. Only the legend direction dispatches.
    it("dispatches no highlight for a hover that came from the chart", () => {
      renderPie();
      mockHoverPayload = { name: "B", value: 30, treePathInfo: [{ name: "" }, { name: "B" }] };
      fireEvent.click(screen.getByTestId("echarts-hover"));
      expect(mockLastHighlight).toBeUndefined();
    });

    // The hole says a value that appears nowhere else in the legend, so it must not be reachable
    // by pointer alone.
    it("does the same for a keyboard focus", () => {
      renderPie();
      fireEvent.focus(screen.getByRole("button", { name: /^A/ }));
      expect(mockLastHighlight).toBe(3);
      expect(holeText()).toEqual(["10", "A"]);
    });

    // A toggled-off entry's slice is not in the tree, so there is no datum to highlight — and the
    // survivors' indexes shift, which is why the legend is built with the same toggle list.
    it("asks for no highlight on a toggled-off entry, and re-indexes the rest", () => {
      renderPie();
      fireEvent.click(screen.getByRole("button", { name: /^B/ })); // toggle B off
      fireEvent.mouseEnter(screen.getByRole("button", { name: /^B/ }));
      expect(mockLastHighlight).toBeUndefined();

      fireEvent.mouseLeave(screen.getByRole("button", { name: /^B/ }));
      fireEvent.mouseEnter(screen.getByRole("button", { name: /^A/ }));
      expect(mockLastHighlight).toBe(1); // A is now the first slice in the tree
    });
  });

  // Pie's `option` is a FUNCTION, and EChartsRenderer re-applies the whole option — for pie, a
  // per-slice canvas measureText pass over every ring — whenever that function's identity
  // changes. It must therefore survive a parent render that changed nothing, and must NOT
  // survive one that changed the config or the data (a stale memo would silently keep drawing
  // the previous chart).
  describe("pie option identity", () => {
    const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];

    /** Re-renders ChartRenderer on a state change of its own parent, exactly as the studio and a
     * dashboard card do, holding the chart's props fixed unless a test varies them. */
    function Harness({ pie, rows }: { pie: Partial<PieConfig>; rows: unknown[][] }) {
      const [, bump] = useState(0);
      return (
        <>
          <button data-testid="bump" onClick={() => bump((n) => n + 1)} />
          <ChartRenderer
            vizType="pie"
            params={params({ pie: { dimensions: ["d"], metric: "m", ...pie } })}
            columns={cols}
            rows={rows}
          />
        </>
      );
    }

    it("keeps the same option across a parent render that changed nothing", () => {
      const rows = [["A", 10], ["B", 30]];
      render(<Harness pie={{}} rows={rows} />);
      const first = mockLastOption;
      // Cleared FIRST, so this cannot pass by the parent's render never reaching the chart at all —
      // the one way "the identity survived" is true and means nothing. It was: deleting the click
      // below left this test green until the clear was added.
      mockLastOption = undefined;
      fireEvent.click(screen.getByTestId("bump"));
      // Same identity => EChartsRenderer's [option] effect doesn't fire => no rebuild.
      expect(mockLastOption).toBe(first);
    });

    it("rebuilds the option when the config changes", () => {
      const rows = [["A", 10], ["B", 30]];
      const { rerender } = render(<Harness pie={{}} rows={rows} />);
      const first = mockLastOption;
      rerender(<Harness pie={{ sliceNames: { A: "Alpha" } }} rows={rows} />);
      expect(mockLastOption).not.toBe(first);
      const built = (mockLastOption as (s: { width: number; height: number }) => {
        series: { data: { name: string }[] }[];
      })({ width: 400, height: 400 });
      expect(built.series[0].data.map((d) => d.name)).toContain("Alpha");
    });

    it("rebuilds the option when the data changes", () => {
      const { rerender } = render(<Harness pie={{}} rows={[["A", 10], ["B", 30]]} />);
      const first = mockLastOption;
      rerender(<Harness pie={{}} rows={[["A", 10], ["B", 30], ["C", 5]]} />);
      expect(mockLastOption).not.toBe(first);
      const built = (mockLastOption as (s: { width: number; height: number }) => {
        series: { data: { name: string }[] }[];
      })({ width: 400, height: 400 });
      expect(built.series[0].data.map((d) => d.name)).toContain("C");
    });
  });

  /**
   * THE SAME BOUNDARY, for the five branches that grew one alongside pie's: `CartesianChart`,
   * `GaugeChart`, `WaterfallChart`, `BoxplotChart` and `SankeyChart`. Table-driven because the
   * argument is identical for each and only the fixture differs — pie stays written out above,
   * since its config arrives by a different route (the whole `params`, not a prebuilt `cfg` prop)
   * and its assertions read slices.
   *
   * Identity is the whole assertion because identity is the whole mechanism: `EChartsRenderer`'s
   * `[option]` effect re-applies the ENTIRE option whenever the function's identity changes, so an
   * inline arrow means a rebuild plus a full `setOption` on every render of whatever renders the
   * chart — a hover, a filter chip, a sibling's state change. And a boundary that stops working is
   * INVISIBLE: the chart still draws the right thing, just at several times the cost, so nothing
   * short of these tests would report it. That is why they exist at all.
   *
   * ALL THREE DIRECTIONS ARE LOAD-BEARING, and direction 1 alone is worth nothing: a memo keyed on
   * a constant holds one identity forever, so it sails through "parent changed nothing" while being
   * completely wrong. Directions 2 and 3 are what say the key is made of something real. That is not
   * a hypothetical — the dependency these boundaries ship with is the SECOND one tried. `[cfg]`
   * memoised nothing at all, because `ChartRenderer` rebuilds every one of these configs from
   * scratch on each render, which is exactly why `Harness` below rebuilds `params` on each of its
   * own: a harness that hoisted `params` out would let `[cfg]` pass and pin the wrong thing.
   *
   * Not covered here, on purpose: `EChartsRenderer` applies the option a SECOND time on mount (its
   * mount effect, then that same `[option]` effect's first run on the same commit — byte-identical,
   * pre-existing, and paid by every function-valued viz). Left alone because the fix reorders shared
   * effects for all viz types and needs its own change and test; Task 5 of the plan doc carries the
   * mechanism. It is mount-only, so it is orthogonal to the per-render cost these tests pin.
   */
  describe("option identity, for the branches that grew a boundary alongside pie's", () => {
    type Built = {
      xAxis?: { data: string[] };
      series: {
        data?: unknown;
        max?: number;
        nodeAlign?: string;
        links?: { source: string; target: string; value: number }[];
      }[];
    };
    /** Reads a captured option the only way a size-function option CAN be read — by calling it.
     *
     * The cast is deliberately UNGUARDED. A branch that reverted to handing over a plain option
     * object would still satisfy every identity assertion below (a memoised object holds its
     * identity exactly as a memoised arrow does), and every content assertion would then read the
     * object's own fields and quietly agree. Calling it instead means that branch dies here, with
     * `o is not a function` — verified by mutation, not assumed. That is a side effect of how this
     * helper reads the option, not a guard designed for the purpose; `SIZE_FUNCTION_VIZ` below is
     * the one that states the shape on purpose. */
    const built = (o: unknown): Built =>
      (o as (s: { width: number; height: number }) => Built)({ width: 640, height: 400 });

    /** One boundary's fixture: a base chart, a viz-setting edit, and a changed row — each of the
     * latter two paired with what it must be VISIBLE as in the rebuilt option. Identity alone would
     * pass for a memo that mints a fresh arrow while still closing over the old inputs, which is a
     * stale chart with a healthy-looking test. */
    type Boundary = {
      what: string;
      vizType: VizType;
      columns: { name: string; type: string }[];
      rows: unknown[][];
      /** Both a different identity and different content from `rows` — direction 3 varies only it. */
      changedRows: unknown[][];
      base: Partial<ChartParams>;
      edited: Partial<ChartParams>;
      expectEdited: (o: Built) => void;
      expectChangedRows: (o: Built) => void;
    };

    const BOUNDARIES: Boundary[] = [
      {
        what: "CartesianChart",
        vizType: "bar",
        columns: [{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }],
        rows: [["a", 10], ["b", 8], ["c", 6]],
        changedRows: [["a", 10], ["b", 8], ["c", 6], ["d", 4]],
        base: { cartesian: { xAxis: "region", yMetrics: ["revenue"] } },
        edited: { cartesian: { xAxis: "region", yMetrics: ["revenue"], maxCategories: 2 } },
        // The fold keeps the largest `maxCategories - 1` and invents "Other" for the rest, so the
        // edit is unmistakable in the categories the builder settled on.
        expectEdited: (o) => expect(o.xAxis!.data).toEqual(["a", "Other"]),
        expectChangedRows: (o) => expect(o.xAxis!.data).toContain("d"),
      },
      {
        what: "GaugeChart",
        vizType: "gauge",
        columns: [{ name: "m", type: "double" }],
        rows: [[100]],
        changedRows: [[500]],
        base: { gauge: { metric: "m" } },
        edited: { gauge: { metric: "m", segments: [{ min: 0, max: 5000, color: "#111111" }] } },
        // Segments ARE the gauge's scale. Unset, it derives one from the value itself (100 -> 200),
        // so a stale option reports 200 where the edited one reports 5000.
        expectEdited: (o) => expect(o.series[0].max).toBe(5000),
        expectChangedRows: (o) => expect(o.series[0].data).toEqual([{ value: 500 }]),
      },
      {
        what: "WaterfallChart",
        vizType: "waterfall",
        columns: [{ name: "step", type: "varchar" }, { name: "delta", type: "double" }],
        rows: [["Q1", 10], ["Q2", -4]],
        changedRows: [["Q1", 10], ["Q2", -4], ["Q3", 7]],
        base: { waterfall: { xAxis: "step", metric: "delta" } },
        edited: { waterfall: { xAxis: "step", metric: "delta", showTotal: false } },
        // The Total is a category the BUILDER invents by summing the steps, so turning it off is
        // visible in the axis itself — a stale option still carries it.
        expectEdited: (o) => expect(o.xAxis!.data).toEqual(["Q1", "Q2"]),
        expectChangedRows: (o) => expect(o.xAxis!.data).toContain("Q3"),
      },
      {
        what: "BoxplotChart",
        vizType: "boxplot",
        columns: [{ name: "grp", type: "varchar" }, { name: "v", type: "double" }],
        // Group B carries an outlier ON PURPOSE: the two whisker types agree on any group without
        // one, so a tidier fixture would make the edit invisible and the assertion below vacuous.
        rows: [["A", 1], ["A", 3], ["B", 5], ["B", 5], ["B", 5], ["B", 5], ["B", 100]],
        changedRows: [["A", 1], ["A", 3], ["B", 5], ["B", 5], ["B", 5], ["B", 5], ["B", 100], ["C", 2]],
        base: { boxplot: { category: "grp", value: "v" } },
        edited: { boxplot: { category: "grp", value: "v", whiskers: "min-max" as const } },
        // Whisker type IS the box's five numbers. Tukey excludes B's 100 and pulls the upper
        // whisker back to the quartile ([5, 5, 5, 5, 5]); min/max reaches the extreme.
        expectEdited: (o) => expect((o.series[0].data as number[][])[1]).toEqual([5, 5, 5, 5, 100]),
        expectChangedRows: (o) => expect(o.xAxis!.data).toContain("C"),
      },
      {
        what: "SankeyChart",
        vizType: "sankey",
        columns: [
          { name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" },
        ],
        rows: [["A", "B", 3]],
        changedRows: [["A", "B", 3], ["B", "C", 2]],
        base: { sankey: { source: "s", target: "t", metric: "v" } },
        edited: { sankey: { source: "s", target: "t", metric: "v", nodeAlign: "right" } },
        expectEdited: (o) => expect(o.series[0].nodeAlign).toBe("right"),
        expectChangedRows: (o) => expect(o.series[0].links).toHaveLength(2),
      },
    ];

    /** Re-renders `ChartRenderer` on a state change of its own parent, as the studio and a dashboard
     * card do. `params` is rebuilt on EVERY render deliberately — that is what every real call site
     * does (ChartExplore's `previewParams()` spreads a fresh object, the dashboard's
     * `applyVizOverride()` returns a new one), and it is the condition under which an identity
     * dependency on the config memoises nothing while looking like it does. `columns` and `rows`
     * come in by prop, so they stay identity-stable unless a test varies them — as the query cache
     * keeps them in production. */
    function Harness({ b, cfg, rows }: { b: Boundary; cfg: "base" | "edited"; rows: unknown[][] }) {
      const [, bump] = useState(0);
      return (
        <>
          <button data-testid="bump" onClick={() => bump((n) => n + 1)} />
          <ChartRenderer
            vizType={b.vizType}
            params={params(cfg === "base" ? b.base : b.edited)}
            columns={b.columns}
            rows={rows}
          />
        </>
      );
    }

    BOUNDARIES.forEach((b) => {
      describe(b.what, () => {
        it("keeps the same option across a parent render that changed nothing", () => {
          render(<Harness b={b} cfg="base" rows={b.rows} />);
          const first = mockLastOption;
          // Cleared FIRST, so the assertion below cannot pass by the parent's render never reaching
          // the chart at all — the one way "the identity survived" is true and means nothing. It
          // has to arrive again, and be the same one.
          mockLastOption = undefined;
          fireEvent.click(screen.getByTestId("bump"));
          // Same identity => EChartsRenderer's [option] effect doesn't fire => no rebuild, no
          // re-apply. This is the assertion an inlined arrow, or a dep list back on `[cfg]`, fails.
          expect(mockLastOption).toBe(first);
        });

        it("rebuilds the option when a viz setting changes", () => {
          const { rerender } = render(<Harness b={b} cfg="base" rows={b.rows} />);
          const first = mockLastOption;
          rerender(<Harness b={b} cfg="edited" rows={b.rows} />);
          expect(mockLastOption).not.toBe(first);
          b.expectEdited(built(mockLastOption));
        });

        it("rebuilds the option when the rows change", () => {
          const { rerender } = render(<Harness b={b} cfg="base" rows={b.rows} />);
          const first = mockLastOption;
          rerender(<Harness b={b} cfg="base" rows={b.changedRows} />);
          expect(mockLastOption).not.toBe(first);
          b.expectChangedRows(built(mockLastOption));
        });
      });
    });
  });

  /**
   * THE SAME SEAM, for the five viz types that hand-listed their fields at this line long after
   * cartesian and pie stopped: gauge, funnel, waterfall, boxplot and sankey.
   *
   * The lists happened to match their config interfaces, so nothing was broken — but that is
   * exactly the state cartesian and pie were both in until a field was added to one side and not
   * the other, at which cost four pie settings (decimals, sliceNames, hiddenSlices, sliceOrder)
   * shipped implemented, panel-exposed and unreachable. These tests exist so the NEXT added field
   * cannot repeat it.
   *
   * They deliberately run through `ChartRenderer` rather than calling the builders, because the
   * builders were never the broken part: `pie.test.ts` calls `buildPieOption` directly and passed
   * throughout, since a direct call bypasses the one line that dropped the fields. The assertion
   * per viz covers EVERY field of its config plus the two the shell contributes, so dropping any
   * single one of them fails on its own:
   *   - `columnSettings` shows up as the `$` prefix (a bare value has none);
   *   - `metricTypes` shows up as the MISSING thousands separator — "varchar" sends the value down
   *     formatValue's string path, where the default "double" would have grouped it.
   */
  describe("-> the remaining option builders' seam", () => {
    /** A metric formatted through both shell settings at once: `$1234.5`, where `$` proves
     * columnSettings arrived and the absent comma proves metricTypes did. */
    const FORMATTED = "$1234.5";
    const settings = { m: { prefix: "$" } };
    const types = { m: "varchar" };

    type Built = {
      series: {
        min?: number; max?: number;
        axisLine?: { lineStyle: { color: [number, string][] } };
        detail?: { formatter: (v: number) => string };
        label?: { formatter: (p: { name: string; value: unknown }) => string };
        data?: unknown;
        links?: { source: string; target: string; value: number }[];
      }[];
      xAxis?: { data: string[] };
      yAxis?: { axisLabel: { formatter: (v: number) => string } };
      tooltip?: { formatter: (p: { dataType?: string; name?: string; value?: unknown }) => string };
    };

    /** The viz types this seam covers, spelled once so the set below and `optionOf` cannot disagree
     * about them — and so a typo in either is a compile error rather than a runtime shape error. */
    type SeamViz = "gauge" | "funnel" | "waterfall" | "boxplot" | "sankey";

    /** The viz types whose `option` prop is a FUNCTION of the measured container size rather than
     * an already-built object — read by CALLING it with a size, the same way `cartesianOption` and
     * `pieOption` above read theirs.
     *
     * LISTED, deliberately, rather than sniffed with `typeof`. A sniff would be self-healing
     * against the exact regression this guards: `buildGaugeOption` and `buildSankeyOption` ignore
     * their size argument today, so a branch reverting to a plain option object produces a
     * byte-identical option and every content assertion below still passes. The waterfall and the
     * box plot DO read theirs now, but only for the x-axis label decision (`interval`, `rotate`,
     * `show`, `grid.bottom`) — none of which any assertion in this block reads either, so the same
     * self-healing would apply to them.
     *
     * The identity block above also dies on such a branch — `built()` calls the captured option
     * with the cast unguarded, so it throws `o is not a function` — but that is a side effect of
     * how it reads the option rather than a guard, and it covers only the vizzes that have a memo
     * boundary. This list is the one place that states the shape ON PURPOSE, and the only cover for
     * a viz that gains a size-function option without gaining an identity test.
     * (Measured, both halves: pointing `GaugeChart` at a plain memoised object fails three tests —
     * this one, and the gauge's two content directions above.) */
    const SIZE_FUNCTION_VIZ = new Set<SeamViz>(["gauge", "sankey", "waterfall", "boxplot"]);

    /** Renders a viz and hands back the option that actually reached EChartsRenderer. */
    const optionOf = (vizType: SeamViz, p: Partial<ChartParams>, columns: { name: string; type: string }[], rows: unknown[][]): Built => {
      render(
        <ChartRenderer
          vizType={vizType}
          params={params({ columnSettings: settings, ...p })}
          columns={columns}
          rows={rows}
          metricTypes={types}
        />,
      );
      return SIZE_FUNCTION_VIZ.has(vizType)
        ? (mockLastOption as (s: { width: number; height: number }) => Built)({ width: 640, height: 400 })
        : (mockLastOption as Built);
    };

    it("forwards every GaugeConfig field, and both shell settings, to the built option", () => {
      // A leading decoy column, so reading the metric by NAME is what produces 1234.5 — dropping
      // `metric` leaves buildGaugeOption with no column at all and a value of 0.
      const o = optionOf(
        "gauge",
        { gauge: { metric: "m", segments: [{ min: 0, max: 1000, color: "#111111" }, { min: 1000, max: 5000, color: "#222222" }] } },
        [{ name: "decoy", type: "double" }, { name: "m", type: "double" }],
        [[999, 1234.5]],
      );
      expect(o.series[0].data).toEqual([{ value: 1234.5 }]);           // metric
      expect([o.series[0].min, o.series[0].max]).toEqual([0, 5000]);    // segments (scale)
      expect(o.series[0].axisLine!.lineStyle.color)                     // segments (zones)
        .toEqual([[0.2, "#111111"], [1, "#222222"]]);
      expect(o.series[0].detail!.formatter(1234.5)).toBe(FORMATTED);    // columnSettings + metricTypes
    });

    // The funnel left ECharts entirely (see FunnelChart.tsx), so this one reads the DOM instead of
    // an option — the seam it guards is the same: every config field, and both shell settings,
    // reaching the renderer.
    it("forwards every FunnelConfig field, and both shell settings, to the funnel", () => {
      render(
        <ChartRenderer
          vizType="funnel"
          params={params({
            columnSettings: settings,
            funnel: { dimension: "d", metric: "m", stepOrder: ["B", "A"], hiddenSteps: ["C"] },
          })}
          columns={[{ name: "d", type: "varchar" }, { name: "decoy", type: "double" }, { name: "m", type: "double" }]}
          rows={[["A", 999, 1234.5], ["B", 999, 10], ["C", 999, 7]]}
          metricTypes={types}
        />,
      );
      // dimension + metric + columnSettings + metricTypes, all at once
      expect(screen.getByText(FORMATTED)).toBeInTheDocument();
      // hiddenSteps
      expect(screen.queryByText("C")).toBeNull();
      // stepOrder — B before A, so B is the first step and the one that names the metric
      const shares = screen.getAllByText(/%$/).map((e) => e.textContent);
      expect(shares).toEqual(["12345.00 %"]);
    });

    it("forwards every WaterfallConfig field, and both shell settings, to the built option", () => {
      const o = optionOf(
        "waterfall",
        { waterfall: { xAxis: "x", metric: "m", showTotal: true } },
        [{ name: "x", type: "varchar" }, { name: "decoy", type: "double" }, { name: "m", type: "double" }],
        [["Q1", 999, 1000], ["Q2", 999, 234.5]],
      );
      // xAxis + metric produce the two steps; showTotal appends the third bar.
      expect(o.xAxis!.data).toEqual(["Q1", "Q2", "Total"]);
      expect(o.yAxis!.axisLabel.formatter(1234.5)).toBe(FORMATTED);
    });

    // Metabase's waterfall renders through its CartesianChart, so a step drills like a bar.
    // egret's branch passed NO click handler, so clicking a waterfall did nothing at all — a seam
    // omission no chart-level test can see, the same shape as the pie's in the studio preview.
    it("drills a waterfall step by its own category", () => {
      const onValueClick = vi.fn();
      mockClickPayload = { name: "Q2", seriesName: "m", value: 5, x: 7, y: 8 };
      render(
        <ChartRenderer
          vizType="waterfall"
          params={params({ waterfall: { xAxis: "x", metric: "m" } })}
          columns={[{ name: "x", type: "varchar" }, { name: "m", type: "double" }]}
          rows={[["Q1", 10], ["Q2", 5]]}
          onValueClick={onValueClick}
        />,
      );
      fireEvent.click(screen.getAllByTestId("echarts")[0]);
      expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "x", value: "Q2" }] }, 7, 8);
    });

    // The Total is a bar the CHART invented by summing the steps. No row holds it, so filtering
    // `= 'Total'` drills to nothing — or to a real category of that name, which is worse because it
    // looks like an answer.
    it("refuses to drill the Total bar", () => {
      const onValueClick = vi.fn();
      mockClickPayload = { name: "Total", seriesName: "m", value: 15, x: 7, y: 8 };
      render(
        <ChartRenderer
          vizType="waterfall"
          params={params({ waterfall: { xAxis: "x", metric: "m" } })}
          columns={[{ name: "x", type: "varchar" }, { name: "m", type: "double" }]}
          rows={[["Q1", 10], ["Q2", 5]]}
          onValueClick={onValueClick}
        />,
      );
      fireEvent.click(screen.getAllByTestId("echarts")[0]);
      expect(onValueClick).not.toHaveBeenCalled();
    });

    // Metabase's box plot drills; egret's branch passed no click handler at all. A `boxplot`
    // element reports no usable `name`, so the category has to come from its `dataIndex` — which
    // is why `EChartsPointClick` had to start carrying one.
    it("drills a box by its own category, from the datum index", () => {
      const onValueClick = vi.fn();
      mockClickPayload = { name: "", seriesName: "", value: [1, 2, 3, 4, 5], dataIndex: 1, x: 7, y: 8 };
      render(
        <ChartRenderer
          vizType="boxplot"
          params={params({ boxplot: { category: "c", value: "m" } })}
          columns={[{ name: "c", type: "varchar" }, { name: "m", type: "double" }]}
          rows={[["A", 1], ["A", 3], ["B", 5], ["B", 9]]}
          onValueClick={onValueClick}
        />,
      );
      fireEvent.click(screen.getAllByTestId("echarts")[0]);
      expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "c", value: "B" }] }, 7, 8);
    });

    // An outlier point carries `[categoryIndex, value]` instead of a datum index; it drills to the
    // same category the box it sits above does.
    it("drills an outlier point to the category it belongs to", () => {
      const onValueClick = vi.fn();
      mockClickPayload = { name: "", seriesName: "", value: 99, data: [0, 99], x: 1, y: 2 };
      render(
        <ChartRenderer
          vizType="boxplot"
          params={params({ boxplot: { category: "c", value: "m" } })}
          columns={[{ name: "c", type: "varchar" }, { name: "m", type: "double" }]}
          rows={[["A", 1], ["A", 3], ["B", 5], ["B", 9]]}
          onValueClick={onValueClick}
        />,
      );
      fireEvent.click(screen.getAllByTestId("echarts")[0]);
      expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "c", value: "A" }] }, 1, 2);
    });

    // Metabase's sankey drills from both kinds of element. An EDGE names both of its ends, so it
    // filters on both columns at once.
    it("drills a sankey edge on both of the columns it joins", () => {
      const onValueClick = vi.fn();
      mockClickPayload = { name: "A > B", dataType: "edge", data: { source: "A", target: "B", value: 3 }, x: 5, y: 6 };
      render(
        <ChartRenderer
          vizType="sankey"
          params={params({ sankey: { source: "s", target: "t", metric: "v" } })}
          columns={[{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }]}
          rows={[["A", "B", 3], ["B", "C", 2]]}
          onValueClick={onValueClick}
        />,
      );
      fireEvent.click(screen.getAllByTestId("echarts")[0]);
      expect(onValueClick).toHaveBeenCalledWith(
        { dims: [{ col: "s", value: "A" }, { col: "t", value: "B" }] }, 5, 6);
    });

    // A node that anything flows INTO is a value of the target column; one that only emits is a
    // value of the source column. The two columns can hold different vocabularies, so picking the
    // wrong one drills to nothing.
    it("drills a sankey node on the column its side of the flow belongs to", () => {
      const cols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
      const rows = [["A", "B", 3], ["B", "C", 2]];
      const drillFor = (name: string) => {
        const onValueClick = vi.fn();
        mockClickPayload = { name, dataType: "node", x: 5, y: 6 };
        render(
          <ChartRenderer
            vizType="sankey"
            params={params({ sankey: { source: "s", target: "t", metric: "v" } })}
            columns={cols} rows={rows} onValueClick={onValueClick}
          />,
        );
        fireEvent.click(screen.getAllByTestId("echarts").slice(-1)[0]);
        return onValueClick.mock.calls[0]?.[0];
      };
      expect(drillFor("A")).toEqual({ dims: [{ col: "s", value: "A" }] });  // only emits
      expect(drillFor("C")).toEqual({ dims: [{ col: "t", value: "C" }] });  // only receives
      expect(drillFor("B")).toEqual({ dims: [{ col: "t", value: "B" }] });  // both — target wins
    });

    it("forwards every BoxplotConfig field, and both shell settings, to the built option", () => {
      const o = optionOf(
        "boxplot",
        { boxplot: { category: "c", value: "m" } },
        [{ name: "c", type: "varchar" }, { name: "decoy", type: "double" }, { name: "m", type: "double" }],
        [["A", 999, 1], ["A", 999, 3], ["B", 999, 5]],
      );
      expect(o.xAxis!.data).toEqual(["A", "B"]);            // category
      expect(o.series[0].data).toEqual([[1, 1.5, 2, 2.5, 3], [5, 5, 5, 5, 5]]); // value
      expect(o.yAxis!.axisLabel.formatter(1234.5)).toBe(FORMATTED);
    });

    it("forwards every SankeyConfig field, and both shell settings, to the built option", () => {
      const o = optionOf(
        "sankey",
        { sankey: { source: "s", target: "t", metric: "m" } },
        [
          { name: "s", type: "varchar" }, { name: "t", type: "varchar" },
          { name: "decoy", type: "double" }, { name: "m", type: "double" },
        ],
        [["A", "B", 999, 1234.5]],
      );
      expect(o.series[0].links).toEqual([{ source: "A", target: "B", value: 1234.5 }]); // source + target + metric
      expect(o.tooltip!.formatter({ dataType: "edge", name: "A > B", value: 1234.5 })).toBe(`A > B: ${FORMATTED}`);
    });
  });
});

describe("cartesian drill — the breakout series", () => {
  const COLS = [
    { name: "region", type: "varchar" },
    { name: "category", type: "varchar" },
    { name: "rev", type: "double" },
  ];
  const ROWS = [["EU", "Widgets", 10], ["EU", "Gadgets", 4], ["US", "Widgets", 7]];

  // `p.seriesName` is the LEGEND's text. Renaming a series therefore made its clicks filter by a
  // string no row holds, and the drill quietly returned nothing.
  it("filters by the series' raw value, not the label the legend shows", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["rev"], series: "category", seriesLabels: { Widgets: "Widgets (renamed)" } } })}
        columns={COLS}
        rows={ROWS}
        onValueClick={onValueClick}
      />,
    );
    mockClickPayload = { name: "EU", seriesName: "Widgets (renamed)", seriesIndex: 0, value: 10, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    const { dims } = onValueClick.mock.calls[0][0];
    expect(dims).toContainEqual({ col: "category", value: "Widgets" });
  });

  it("attaches the breakout dimension's temporal unit so it drills to a period", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="bar"
        params={params({
          cartesian: { xAxis: "region", yMetrics: ["rev"], series: "order_date" },
          spec: {
            metrics: ["rev"], dimensions: ["region", "order_date"], columns: [], filters: [],
            orderby: [], row_limit: 1000, rollup_groups: [],
            temporal_units: { order_date: "month" },
          },
        })}
        columns={[{ name: "region", type: "varchar" }, { name: "order_date", type: "date" }, { name: "rev", type: "double" }]}
        rows={[["EU", "2026-01-01", 10]]}
        onValueClick={onValueClick}
      />,
    );
    mockClickPayload = { name: "EU", seriesName: "2026-01-01", seriesIndex: 0, value: 10, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    const { dims } = onValueClick.mock.calls[0][0];
    expect(dims).toContainEqual({ col: "order_date", value: "2026-01-01", unit: "month" });
  });
});

describe("cartesian drill — the folded Other bar", () => {
  const COLS = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const ROWS = [["EU", 100], ["US", 80], ["APAC", 5], ["LATAM", 3]];

  // `maxCategories` sums the tail into a bar the chart invented. `= 'Other'` matches no row —
  // and when a real category of that name exists, foldTail merges into it, so the filter can
  // return a fraction of the bar and pass for the whole of it.
  it("refuses the bar the long-tail fold invented", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["rev"], maxCategories: 3 } })}
        columns={COLS} rows={ROWS} onValueClick={onValueClick}
      />,
    );
    mockClickPayload = { name: "Other", seriesName: "rev", seriesIndex: 0, value: 8, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).not.toHaveBeenCalled();

    // A real category still drills.
    mockClickPayload = { name: "EU", seriesName: "rev", seriesIndex: 0, value: 100, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "region", value: "EU" }] }, 1, 2);
  });

  // With no fold configured, a column whose value genuinely IS "Other" must keep drilling.
  it("still drills a real category named Other when nothing was folded", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["rev"] } })}
        columns={COLS} rows={[["Other", 12]]} onValueClick={onValueClick}
      />,
    );
    mockClickPayload = { name: "Other", seriesName: "rev", seriesIndex: 0, value: 12, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "region", value: "Other" }] }, 1, 2);
  });
});
