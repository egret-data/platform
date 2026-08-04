import { describe, it, expect } from "vitest";
import { buildWaterfallOption } from "../options/waterfall";

const cols = [{ name: "m", type: "varchar" }, { name: "delta", type: "double" }];
/**
 * `Q1/Q2/Q3`, not the `Jan/Feb/Mar` this fixture used to carry. A nominal step axis is now sorted
 * (see the `nominal step order` suite below), and month NAMES in a varchar column sort
 * alphabetically — Feb before Jan — which is the honest cost of that rule and would have made every
 * assertion in this suite read as if it were about ordering. These three sort into the order they
 * are written in, so each test below is about the thing it names. A waterfall bucketed by an actual
 * date column takes the temporal branch and is unaffected.
 */
const rows = [["Q1", 100], ["Q2", -30], ["Q3", 50]];

const noTotal = { xAxis: "m", metric: "delta", showTotal: false };

describe("buildWaterfallOption", () => {
  it("floats each delta on a transparent base = the running total", () => {
    const opt = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(opt.series[0].data).toEqual([0, 70, 70]);                       // base = min(before, after)
    expect(opt.series[1].data.map((d: any) => d.value)).toEqual([100, 30, 50]); // |delta|
    expect(opt.xAxis.data).toEqual(["Q1", "Q2", "Q3"]);
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
    expect(shown.xAxis.data).toEqual(["Q1", "Q2", "Q3", "Total"]);
    expect(shown.series[0].data[3]).toBe(0);
    expect(shown.series[1].data[3].value).toBe(120); // 100 - 30 + 50
    const hidden = buildWaterfallOption(cols, rows, noTotal) as any;
    expect(hidden.xAxis.data).toEqual(["Q1", "Q2", "Q3"]);
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
    const html = opt.tooltip.formatter([{ seriesIndex: 1, dataIndex: 1, name: "Q2" }]);
    expect(html).toContain("Q2");
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

/**
 * A NOMINAL step axis, the fourth chart to reach PR #394's defect: every stored spec carries
 * `orderby: []`, so the row order is whatever the engine happened to return — measured across ten
 * calls to the same endpoint as `['Widget','Gadget','Doohickey','Gizmo']` on seven and
 * `['Gizmo','Widget','Gadget','Doohickey']` on the other three, with three consecutive page loads
 * all landing on the same one (which is why a two-load comparison called it deterministic).
 *
 * WORSE HERE THAN ON A BAR CHART, which is why the waterfall does not get the funnel's exemption:
 * the running total builds in this order, so an arbitrary arrival order does not merely relabel the
 * bars, it changes every cumulative value drawn. Nothing in `WaterfallConfig` lets an author state
 * a step order the way `funnel.stepOrder` does — the only channel is the query's own sort, which
 * `preserveCategoryOrder` below honours.
 *
 * THE FIXTURES ARE THE POINT: both are the measured orders, NEITHER is alphabetical, and they
 * differ from each other, so a fixture already in the target order could not detect this.
 */
describe("buildWaterfallOption nominal step order", () => {
  const ncols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
  const ARRIVAL_A = [["Widget", 40], ["Gadget", 30], ["Doohickey", 10], ["Gizmo", 20]];
  const ARRIVAL_B = [["Gizmo", 20], ["Widget", 40], ["Gadget", 30], ["Doohickey", 10]];
  const ALPHABETICAL = ["Doohickey", "Gadget", "Gizmo", "Widget"];
  const wf = (rows: unknown[][], preserve?: boolean) =>
    buildWaterfallOption(ncols, rows, {
      xAxis: "category", metric: "revenue", preserveCategoryOrder: preserve,
    }) as any;

  it("orders nominal steps ascending whatever order the engine returned them in", () => {
    for (const rows of [ARRIVAL_A, ARRIVAL_B]) {
      const opt = wf(rows);
      expect(opt.xAxis.data).toEqual([...ALPHABETICAL, "Total"]);
      // …AND THE RUNNING TOTAL BUILDS IN THAT ORDER. Asserting only the axis passes on a half-fix
      // that re-labels the bars while summing the arrival order — the deltas below would then be
      // 40/30/10/20 under one label set, drawing every bar at the wrong height.
      expect(opt.series[1].data.map((d: any) => d.value)).toEqual([10, 30, 20, 40, 100]);
      // The transparent base is the cumulative sum BEFORE each step: 0, 10, 40, 60, and 0 for the
      // Total, which rises from the axis.
      expect(opt.series[0].data).toEqual([0, 10, 40, 60, 0]);
      // The tooltip's own number, from the same `runningTotals` the base is derived from.
      expect(opt.tooltip.formatter([{ seriesIndex: 1, dataIndex: 2, name: "Gizmo" }]))
        .toContain("Running total: 60");
    }
  });

  /**
   * The other side of the rule, and the reason sorting here is a DEFAULT rather than an override:
   * PR #395 gave the studio a sort control, and the waterfall descriptor passes `shared.orderby`
   * through untouched, so it is offered one. A chart whose author asked for an order has the engine
   * return the rows in it, and `ChartRenderer` sets this flag so the client does not sort it away.
   */
  it("keeps the arrival order — and its cumulative values — when the query already sorted", () => {
    for (const rows of [ARRIVAL_A, ARRIVAL_B]) {
      const opt = wf(rows, true);
      expect(opt.xAxis.data).toEqual([...rows.map((r) => r[0]), "Total"]);
      expect(opt.series[1].data.map((d: any) => d.value))
        .toEqual([...rows.map((r) => r[1]), 100]);
      // The base under each step is the total BEFORE it, so it is the arrival order's own
      // cumulative sum — a preserved axis over re-summed values fails here rather than passing.
      const deltas = rows.map((r) => r[1] as number);
      const before = deltas.map((_, i) => deltas.slice(0, i).reduce((s, v) => s + v, 0));
      expect(opt.series[0].data).toEqual([...before, 0]);
    }
  });

  // A numeric or temporal step axis is not a nominal one: chronology and magnitude are not opinions
  // a query can override, and `sortCategories` orders those branches whatever `preserve` says.
  it("still orders a numeric step column even when asked to preserve", () => {
    const bcols = [{ name: "bucket", type: "int" }, { name: "v", type: "double" }];
    const opt = buildWaterfallOption(bcols, [["10", 5], ["9", 1], ["2", 2]], {
      xAxis: "bucket", metric: "v", showTotal: false, preserveCategoryOrder: true,
    }) as any;
    expect(opt.xAxis.data).toEqual(["2", "9", "10"]);
    expect(opt.series[1].data.map((d: any) => d.value)).toEqual([2, 1, 5]);
  });
});
