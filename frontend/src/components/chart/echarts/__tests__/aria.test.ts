import { describe, it, expect } from "vitest";
import { ariaDescription } from "../base";
import { buildPieOption } from "../options/pie";
import { buildCartesianOption } from "../options/cartesian";
import { registerEChartsModules } from "../register";

const fmt = (v: unknown) => `${Number(v).toLocaleString()}`;

describe("ariaDescription", () => {
  it("says what the chart is and reads the values with the chart's own formatting", () => {
    const a = ariaDescription({
      kind: "Bar chart", metric: "total_revenue", dimension: "region", format: fmt,
      points: [{ name: "North", value: 27880 }, { name: "South", value: 30137 }],
    });
    expect(a.label.description).toBe(
      "Bar chart of total_revenue by region. 2 values: North 27,880, South 30,137.",
    );
  });

  it("keeps the decal patterns and the enabled flag", () => {
    const a = ariaDescription({ kind: "Pie chart", format: fmt, points: [] });
    expect(a.enabled).toBe(true);
    expect(a.decal).toEqual({ show: true });
  });

  it("mentions the series count when more than one is plotted", () => {
    const a = ariaDescription({
      kind: "Line chart", format: fmt, seriesCount: 3,
      points: [{ name: "Jan", value: 1 }],
    });
    expect(a.label.description).toContain("3 series;");
  });

  it("caps the enumeration instead of reading hundreds of values", () => {
    const points = Array.from({ length: 25 }, (_, i) => ({ name: `c${i}`, value: i }));
    const a = ariaDescription({ kind: "Bar chart", format: fmt, points });
    expect(a.label.description).toContain("and 5 more");
  });

  it("says so when there is nothing to describe", () => {
    expect(ariaDescription({ kind: "Bar chart", format: fmt, points: [] }).label.description)
      .toContain("No data.");
  });
});

describe("charts carry a description rather than ECharts' generated one", () => {
  const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["North", 10], ["South", 20]];

  it("pie", () => {
    const o = buildPieOption(columns, rows, { dimensions: ["region"], metric: "rev" }) as {
      aria: { label: { description: string } };
    };
    expect(o.aria.label.description).toContain("Pie chart of rev by region");
  });

  it("cartesian", () => {
    const o = buildCartesianOption(columns, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"] }) as {
      aria: { label: { description: string } };
    };
    expect(o.aria.label.description).toContain("Bar chart");
    // ECharts' own text reads "the data for North is 0, 10" — index first. Ours does not.
    expect(o.aria.label.description).not.toMatch(/is \d+, \d+/);
  });
});

describe("the aria component is registered", () => {
  // Without it ECharts silently ignores every `aria` option: no description, no decals.
  // That was the state of the app until this audit.
  it("registers without throwing and is idempotent", () => {
    expect(() => { registerEChartsModules(); registerEChartsModules(); }).not.toThrow();
  });
});
