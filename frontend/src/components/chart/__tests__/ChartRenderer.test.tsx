// src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { useState } from "react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams, PieConfig } from "@/api/charts";

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

  // The pie is ONE `sunburst` series, so a click carries no per-ring `seriesName` to attribute it
  // by — its ring comes from the depth of `treePathInfo`, whose first entry is the virtual root
  // ECharts wraps `series.data` in. One case per ring, since getting the off-by-one wrong would
  // drill every slice on the wrong column.
  it("drills a pie click on the clicked ring's own dimension, not always dimensions[0]", () => {
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
    const root = { name: "" };

    // Ring 0 — path is [root, EU].
    mockClickPayload = { name: "EU", value: 40, treePathInfo: [root, { name: "EU" }], x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "region", value: "EU" }] }, 1, 2);

    // Ring 1 — must drill on "category", not "region", even though the slice is still one level
    // out from a ring-0 wedge.
    mockClickPayload = {
      name: "Widgets", value: 10,
      treePathInfo: [root, { name: "EU" }, { name: "Widgets" }], x: 5, y: 6,
    };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "category", value: "Widgets" }] }, 5, 6);

    // Ring 2.
    mockClickPayload = {
      name: "Direct", value: 10,
      treePathInfo: [root, { name: "EU" }, { name: "Widgets" }, { name: "Direct" }], x: 7, y: 8,
    };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "channel", value: "Direct" }] }, 7, 8);

    // A click with no path at all (nothing else in the app sends one) still drills the first
    // dimension rather than throwing or indexing off the end.
    mockClickPayload = { name: "EU", value: 40, x: 9, y: 10 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "region", value: "EU" }] }, 9, 10);
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
  it("forwards cartesian.maxCategories through to the built option", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["revenue"], maxCategories: 3 } })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["a", 10], ["b", 8], ["c", 6], ["d", 4], ["e", 2]]}
      />,
    );
    const data = (mockLastOption as { xAxis: { data: string[] } }).xAxis.data;
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
    const names = (mockLastOption as { series: { name?: string }[] }).series.map((s) => s.name);
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
      // Pie is the one viz whose `option` prop is a function of the measured container size.
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

    /** Renders a viz and hands back the option that actually reached EChartsRenderer. */
    const optionOf = (vizType: "gauge" | "funnel" | "waterfall" | "boxplot" | "sankey", p: Partial<ChartParams>, columns: { name: string; type: string }[], rows: unknown[][]): Built => {
      render(
        <ChartRenderer
          vizType={vizType}
          params={params({ columnSettings: settings, ...p })}
          columns={columns}
          rows={rows}
          metricTypes={types}
        />,
      );
      return mockLastOption as Built;
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

    it("forwards every FunnelConfig field, and both shell settings, to the built option", () => {
      const o = optionOf(
        "funnel",
        { funnel: { dimension: "d", metric: "m" } },
        [{ name: "d", type: "varchar" }, { name: "decoy", type: "double" }, { name: "m", type: "double" }],
        [["A", 999, 1234.5], ["B", 999, 10]],
      );
      expect(o.series[0].data).toEqual([{ name: "A", value: 1234.5 }, { name: "B", value: 10 }]); // dimension + metric
      expect(o.series[0].label!.formatter({ name: "A", value: 1234.5 })).toBe(`A: ${FORMATTED}`);
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
