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
});
