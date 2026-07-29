// src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams } from "@/api/charts";

// Captures the `option` prop each render passes to EChartsRenderer, so tests can assert on
// what actually reaches the renderer — the exact seam where four settings (maxCategories,
// compactValues, seriesOrder, hiddenSeries) were once silently dropped by a hand-maintained
// field whitelist in ChartRenderer.
let mockLastOption: unknown;

vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option, onPointClick }: { option: unknown; onPointClick?: (p: { name?: string; seriesName?: string; value?: unknown; x: number; y: number }) => void }) => {
    mockLastOption = option;
    return <button data-testid="echarts" onClick={() => onPointClick?.({ name: "EU", seriesName: "rev", value: 10, x: 3, y: 4 })} />;
  },
}));

const params = (p: Partial<ChartParams> = {}): ChartParams => ({
  spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
  ...p,
});

describe("ChartRenderer", () => {
  beforeEach(() => {
    mockLastOption = undefined;
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
});
