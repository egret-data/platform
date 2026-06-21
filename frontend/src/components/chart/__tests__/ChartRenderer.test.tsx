// src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams } from "@/api/charts";

vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: () => <div data-testid="echarts" />,
}));

const params = (p: Partial<ChartParams> = {}): ChartParams => ({
  spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
  ...p,
});

describe("ChartRenderer", () => {
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
        params={params({ gauge: { metric: "revenue", min: 0, max: 100 } })}
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
});
