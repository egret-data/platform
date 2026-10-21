import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * KI-045: a row chart sorted largest-first drew its largest bar at the BOTTOM — ECharts lays a
 * vertical category axis out from the bottom up — so a "top 10" read upside down. The category axis
 * is inverted so the rows read top to bottom in the order the query returned them, as Metabase's
 * row chart does. Where each label lands is checked in the browser; here, that the axis is the
 * inverted one and that the data order is untouched by it.
 */
const cols = [{ name: "product", type: "varchar" }, { name: "orders", type: "double" }];
const rows: unknown[][] = [["Brand#32", 268], ["Brand#53", 266], ["Brand#45", 175]];

type Axis = { type?: string; inverse?: boolean; data?: unknown[] };
const one = (a: Axis | Axis[]) => (Array.isArray(a) ? a[0] : a);
const build = (cfg: Partial<CartesianBuildConfig>) =>
  buildCartesianOption(cols, rows, { xAxis: "product", yMetrics: ["orders"], preserveCategoryOrder: true, ...cfg } as CartesianBuildConfig) as unknown as { xAxis: Axis | Axis[]; yAxis: Axis | Axis[] };

describe("a row chart reads top to bottom", () => {
  it("inverts its category axis and keeps the query's order", () => {
    const y = one(build({ kind: "row" }).yAxis);
    expect(y.type).toBe("category");
    expect(y.inverse).toBe(true);
    expect(y.data).toEqual(["Brand#32", "Brand#53", "Brand#45"]);
  });

  it("leaves a vertical bar chart's category axis alone", () => {
    expect(one(build({ kind: "bar" }).xAxis).inverse).toBeUndefined();
  });
});
