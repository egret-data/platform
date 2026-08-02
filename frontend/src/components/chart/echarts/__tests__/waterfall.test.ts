import { describe, it, expect } from "vitest";
import { buildWaterfallOption } from "../options/waterfall";

const cols = [{ name: "m", type: "varchar" }, { name: "delta", type: "double" }];
const rows = [["Jan", 100], ["Feb", -30], ["Mar", 50]];

const noTotal = { xAxis: "m", metric: "delta", showTotal: false };

describe("buildWaterfallOption", () => {
  it("floats each delta on a transparent base = the running total", () => {
    const opt = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(opt.series[0].data).toEqual([0, 70, 70]);                       // base = min(before, after)
    expect(opt.series[1].data.map((d: any) => d.value)).toEqual([100, 30, 50]); // |delta|
    expect(opt.xAxis.data).toEqual(["Jan", "Feb", "Mar"]);
  });

  // Both series must stack ALL values, not ECharts' default same-sign rule — see the builder's
  // note. A base that goes negative otherwise leaves its own stack and every later bar is drawn
  // from the axis. `waterfall.echarts.test.tsx` proves it against the real renderer; this pins the
  // switch itself so it cannot be dropped in an edit.
  it("stacks across signs on both series", () => {
    const opt = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(opt.series.map((s: any) => s.stackStrategy)).toEqual(["all", "all"]);
  });

  it("colours increases and decreases differently", () => {
    const opt = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(opt.series[1].data[0].itemStyle.color).not.toBe(opt.series[1].data[1].itemStyle.color);
  });

  it("takes the increase, decrease and total colours from the config", () => {
    const opt = buildWaterfallOption(cols, rows, {
      xAxis: "m", metric: "delta", increaseColor: "#111111", decreaseColor: "#222222", totalColor: "#333333",
    }) as any;
    expect(opt.series[1].data.map((d: any) => d.itemStyle.color))
      .toEqual(["#111111", "#222222", "#111111", "#333333"]);
  });

  // Metabase's `waterfall.show_total` defaults to TRUE. egret's read a bare truthy check, so a
  // chart nobody had configured drew no total at all.
  it("shows the Total bar unless it is turned off", () => {
    const shown = buildWaterfallOption(cols, rows, { xAxis: "m", metric: "delta" }) as any;
    expect(shown.xAxis.data).toEqual(["Jan", "Feb", "Mar", "Total"]);
    expect(shown.series[0].data[3]).toBe(0);
    expect(shown.series[1].data[3].value).toBe(120); // 100 - 30 + 50
    const hidden = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(hidden.xAxis.data).toEqual(["Jan", "Feb", "Mar"]);
  });

  // The bar's value is a MAGNITUDE, because it is drawn upward from a floating base. Anything that
  // reports the step to a reader has to re-apply the sign, or a fall of 30 is announced as a rise.
  it("keeps the sign in the data label and in the aria description", () => {
    const opt = buildWaterfallOption(cols, rows, { ...noTotal, showLabels: true }) as any;
    expect(opt.series[1].label.formatter({ dataIndex: 1 })).toContain("-30");
    expect(opt.aria.label.description).toContain("-30");
  });

  // The shared axis tooltip reports the SERIES' value, and a waterfall bar's value is a magnitude
  // — it read "Q2  60" for a fall of 60. Metabase reads the original signed datum instead.
  it("reports the signed change and the running total in the tooltip", () => {
    const opt = buildWaterfallOption(cols, rows, noTotal) as any;
    const html = opt.tooltip.formatter([{ seriesIndex: 1, dataIndex: 1, name: "Feb" }]);
    expect(html).toContain("Feb");
    expect(html).toContain("-30");
    expect(html).toContain("Running total: 70"); // 100 - 30
  });

  it("names the total in the tooltip without claiming it is a step's change", () => {
    const opt = buildWaterfallOption(cols, rows, { xAxis: "m", metric: "delta" }) as any;
    const html = opt.tooltip.formatter([{ seriesIndex: 1, dataIndex: 3, name: "Total" }]);
    expect(html).toContain("120");
    expect(html).not.toContain("Running total");
  });

  it("empty when x or metric is missing", () => {
    expect((buildWaterfallOption(cols, rows, {}) as any).series[1].data).toEqual([]);
  });

  it("orders temporal steps chronologically regardless of row order (Metabase parity)", () => {
    const dcols = [{ name: "d", type: "date" }, { name: "v", type: "double" }];
    const unordered = [["2024-03-01", 5], ["2024-01-01", 10], ["2024-02-01", -3]];
    const opt = buildWaterfallOption(dcols, unordered, { xAxis: "d", metric: "v", showTotal: false }) as any;
    expect(opt.xAxis.data).toEqual(["2024-01-01", "2024-02-01", "2024-03-01"]);
    expect(opt.series[0].data).toEqual([0, 7, 7]); // base: Jan 0, Feb min(10,7)=7, Mar min(7,12)=7
  });

  it("titles both axes from the columns, and lets the config override or hide either", () => {
    const auto = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(auto.xAxis.name).toBe("m");
    expect(auto.yAxis.name).toBe("delta");
    const named = buildWaterfallOption(cols, rows, { ...noTotal, xLabel: "Month", showYLabel: false }) as any;
    expect(named.xAxis.name).toBe("Month");
    expect(named.yAxis.name).toBeUndefined();
  });

  // Metabase folds label rotation into its "Show lines and tick marks" select. egret tilted long
  // names at 30° on a guess and had no way to be told otherwise.
  it("takes an explicit label rotation over its own guess", () => {
    const auto = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(auto.xAxis.axisLabel.rotate).toBe(0); // three short names: no tilt
    const tilted = buildWaterfallOption(cols, rows, { ...noTotal, xLabelRotation: 90 }) as any;
    expect(tilted.xAxis.axisLabel.rotate).toBe(90);
    // …and takes no room under the plot for it. This used to reserve 44px, on the reasoning that a
    // steeper tilt needs more room or the names are clipped — true of the room, wrong about who
    // reserves it. `containLabel: true` already expands the grid around the rotated labels, so the
    // 44 was a second reservation of the same band and came off the plot instead; see
    // `cartesianXLabels.test.ts` for the contract and `containLabelRotation.echarts.test.ts` for
    // the measurement against real ECharts.
    expect(tilted.grid.bottom).toBe(auto.grid.bottom);
  });

  it("draws a goal line when one is set", () => {
    const opt = buildWaterfallOption(cols, rows, { ...noTotal, goalValue: 90, goalLabel: "Target" }) as any;
    expect(opt.series[1].markLine.data).toEqual([{ yAxis: 90 }]);
    expect(opt.series[1].markLine.label.formatter).toBe("Target");
    expect((buildWaterfallOption(cols, rows, noTotal) as any).series[1].markLine).toBeUndefined();
  });
});
