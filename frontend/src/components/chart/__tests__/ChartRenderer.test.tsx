// src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams, PieConfig } from "@/api/charts";

// Captures the `option` prop each render passes to EChartsRenderer, so tests can assert on
// what actually reaches the renderer — the exact seam where four settings (maxCategories,
// compactValues, seriesOrder, hiddenSeries) were once silently dropped by a hand-maintained
// field whitelist in ChartRenderer.
let mockLastOption: unknown;
// Configurable so a test can simulate a click landing on a specific ring/series; defaults match
// what every pre-existing test here expects.
let mockClickPayload: { name?: string; seriesName?: string; value?: unknown; x: number; y: number } = {
  name: "EU", seriesName: "rev", value: 10, x: 3, y: 4,
};

vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option, onPointClick }: { option: unknown; onPointClick?: (p: { name?: string; seriesName?: string; value?: unknown; x: number; y: number }) => void }) => {
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

  it("drills a pie click on the clicked ring's own dimension, not always dimensions[0]", () => {
    const onValueClick = vi.fn();
    render(
      <ChartRenderer
        vizType="pie"
        params={params({ pie: { dimensions: ["region", "category"], metric: "rev" } })}
        columns={[
          { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "rev", type: "double" },
        ]}
        rows={[["EU", "Widgets", 10]]}
        onValueClick={onValueClick}
      />,
    );
    // Inner ring: its ECharts series is named after dimensions[0] ("region").
    mockClickPayload = { name: "EU", seriesName: "region", value: 40, x: 1, y: 2 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "region", value: "EU" }] }, 1, 2);

    // Outer ring: its series is named after dimensions[1] ("category") -- must drill on
    // "category", not "region", even though it's still dimensions[0] positionally.
    mockClickPayload = { name: "Widgets", seriesName: "category", value: 10, x: 5, y: 6 };
    fireEvent.click(screen.getByTestId("echarts"));
    expect(onValueClick).toHaveBeenLastCalledWith({ dims: [{ col: "category", value: "Widgets" }] }, 5, 6);
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
});
