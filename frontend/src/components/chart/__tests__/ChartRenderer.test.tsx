// src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
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

vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option, onPointClick }: { option: unknown; onPointClick?: (p: MockClick) => void }) => {
    mockLastOption = option;
    return <button data-testid="echarts" onClick={() => onPointClick?.(mockClickPayload)} />;
  },
}));

const params = (p: Partial<ChartParams> = {}): ChartParams => ({
  spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
  ...p,
});

describe("ChartRenderer", () => {
  beforeEach(() => {
    mockLastOption = undefined;
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
      // A is 10 of this fixture's 45, i.e. 22.22...% — so 0 decimals renders visibly differently
      // from the default 2.
      const labelOfA = (pie: Partial<PieConfig>) =>
        pieOption(pie).series[0].data.find((d) => d.name === "A")!.label.formatter();
      expect(labelOfA({ percentVisibility: "inside", showLabels: false })).toBe("22.22%");
      expect(labelOfA({ percentVisibility: "inside", showLabels: false, decimals: 0 })).toBe("22%");
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
});
