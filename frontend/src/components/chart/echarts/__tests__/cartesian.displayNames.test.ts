import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * KI-050: a series with no breakout is named after its metric, and the name it is DRAWN under —
 * the legend entry and the tooltip row — is the metric's display name (`column_title`, which the
 * editor fills from the dataset's display name), the same name the axis title already uses. It
 * read `revenue: 434,187,711.87` under a "Revenue" axis title.
 */
const cols = [{ name: "priority", type: "varchar" }, { name: "revenue", type: "double" }, { name: "orders", type: "bigint" }];
const rows: unknown[][] = [["1-URGENT", 10, 1], ["2-HIGH", 20, 2]];
type Built = { series: { name: string; __raw?: string }[]; tooltip: { formatter: (p: unknown) => string } };
const build = (extra: Partial<CartesianBuildConfig>) =>
  buildCartesianOption(cols, rows, { kind: "bar", xAxis: "priority", yMetrics: ["revenue"], ...extra } as CartesianBuildConfig) as unknown as Built;
const param = (seriesName: string, value: number) =>
  ({ axisValueLabel: "1-URGENT", name: "1-URGENT", seriesName, value, marker: "●", dataIndex: 0 });

describe("series names use the metric's display name", () => {
  it("names the series and its tooltip row by column_title", () => {
    const o = build({ columnSettings: { revenue: { column_title: "Revenue", decimals: 2 } } });
    expect(o.series.map((s) => s.name)).toStrictEqual(["Revenue"]);
    // The raw name stays the series' identity (colours, drill, settings rows key on it).
    expect(o.series[0].__raw).toBe("revenue");
    // And the row still finds its metric's format through the display name.
    expect(o.tooltip.formatter([param("Revenue", 10)])).toContain("Revenue: <b>10.00</b>");
  });

  it("an explicit series rename still wins over the display name", () => {
    const o = build({ columnSettings: { revenue: { column_title: "Revenue" } }, seriesLabels: { revenue: "Sales" } });
    expect(o.series.map((s) => s.name)).toStrictEqual(["Sales"]);
  });

  it("names each of two metrics by its own display name, and keeps the raw name without one", () => {
    const o = build({ yMetrics: ["revenue", "orders"], columnSettings: { revenue: { column_title: "Revenue" } } });
    expect(o.series.map((s) => s.name)).toStrictEqual(["Revenue", "orders"]);
  });
});
