import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [
  { name: "region", type: "varchar" },
  { name: "category", type: "varchar" },
  { name: "total_revenue", type: "double" },
  { name: "order_count", type: "double" },
];
const rows = [["North", "A", 10, 1], ["South", "B", 20, 2]];
const opt = (cfg: any) => buildCartesianOption(columns, rows, cfg) as any;

describe("cartesian defaults, applied at render time", () => {
  it("hides the legend for a single metric with no breakout", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"] }).legend).toBeUndefined();
  });

  it("shows it once there is a breakout", () => {
    expect(opt({ kind: "bar", xAxis: "region", series: "category", yMetrics: ["total_revenue"] }).legend)
      .toBeDefined();
  });

  it("shows it for two metrics", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue", "order_count"] }).legend)
      .toBeDefined();
  });

  it("still obeys an explicit choice in both directions", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue", "order_count"], showLegend: false }).legend)
      .toBeUndefined();
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"], showLegend: true }).legend)
      .toBeDefined();
  });

  it("titles the axes from the column and metric names", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"] });
    expect(o.xAxis.name).toBe("region");
    expect(o.yAxis.name).toBe("total_revenue");
  });

  it("prefers a renamed column over its raw name", () => {
    const o = opt({
      kind: "bar", xAxis: "region", yMetrics: ["total_revenue"],
      columnSettings: {
        region: { column_title: "Sales region" },
        total_revenue: { column_title: "Revenue" },
      },
    });
    expect(o.xAxis.name).toBe("Sales region");
    expect(o.yAxis.name).toBe("Revenue");
  });

  it("lets a typed title win, and an empty string mean 'no title'", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"], xLabel: "Region" }).xAxis.name)
      .toBe("Region");
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"], xLabel: "" }).xAxis.name)
      .toBe("");
  });

  it("leaves the value axis untitled when it plots more than one metric", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue", "order_count"] });
    expect(o.yAxis.name).toBeUndefined();
  });

  it("titles a dual axis from the metrics that axis actually plots", () => {
    const o = opt({
      kind: "combo", xAxis: "region",
      yMetrics: ["order_count", "total_revenue"],
      rightAxisMetrics: ["order_count"],
    });
    expect(o.yAxis[0].name).toBe("total_revenue");
    expect(o.yAxis[1].name).toBeUndefined();
  });

  it("follows a renamed series, which is what the legend already shows", () => {
    const o = opt({
      kind: "bar", xAxis: "region", yMetrics: ["total_revenue"],
      seriesLabels: { total_revenue: "Revenue" },
    });
    expect(o.yAxis.name).toBe("Revenue");
  });

  it("puts each title on the edge it renders on when horizontal", () => {
    const o = opt({ kind: "row", xAxis: "region", yMetrics: ["total_revenue"] });
    expect(o.yAxis.name).toBe("region");
    expect(o.xAxis.name).toBe("total_revenue");
    expect(o.grid.left).toBeGreaterThan(o.grid.right);
  });

  it("treats an empty yLabel as 'no title' too", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"], yLabel: "" }).yAxis.name).toBe("");
  });
});
