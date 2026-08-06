import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

/**
 * WHY THE AXES TAB WITHDRAWS ITS TWO-METRIC CONTROLS UNDER A BREAKOUT.
 *
 * This file is a PIN, not a bug test: every assertion below passed before the panel change that
 * cites it and passes after, deliberately. `settings/CartesianDisplaySection.tsx` now hides the
 * per-metric Left/Right selects and the "Split y-axis when necessary" switch whenever
 * `config.series` is set, and the reason it gives is a claim about THIS builder. A panel gate whose
 * stated reason lives only in a comment is one refactor away from being wrong in a way nobody
 * notices — so the claim is asserted where it can fail.
 *
 * If a later change teaches the breakout branch about two y-axes (the real capability gap: egret's
 * left/right choice is per-METRIC, so it cannot split BANDS the way Metabase can), these tests are
 * the ones that should fail, and the panel gate should be reopened in the same PR.
 */
const cols = [
  { name: "x", type: "varchar" },
  { name: "g", type: "varchar" },
  { name: "v", type: "double" },
  { name: "w", type: "double" },
];
// `w` is a thousand times `v`, which is exactly the shape `shouldAutoSplitY` exists to catch: the
// smaller series' range is well under 5% of the chart's, so on a chart WITHOUT a breakout this data
// splits by itself. That makes the breakout assertions below about the breakout rather than about a
// split that was never going to happen.
const rows = [["a", "L", 1, 1000], ["a", "R", 2, 2000], ["b", "L", 3, 3000], ["b", "R", 4, 4000]];
const build = (extra: Record<string, unknown>) =>
  buildCartesianOption(cols, rows, {
    kind: "bar", xAxis: "x", yMetrics: ["v", "w"], ...extra,
  } as never) as never as { series: { name?: string; yAxisIndex?: number }[]; yAxis: unknown };

describe("a breakout ignores both of the two-metric axis controls", () => {
  it("draws one band per series value on axis 0, whatever rightAxisMetrics names", () => {
    const opt = build({ series: "g", rightAxisMetrics: ["w"] });
    // The bands are the SERIES values, not the metrics: `w` is not plotted at all, so naming it as
    // the right-axis metric names something that is not on the chart.
    expect(opt.series.map((s) => s.name)).toEqual(["L", "R"]);
    expect(opt.series.every((s) => s.yAxisIndex == null || s.yAxisIndex === 0)).toBe(true);
  });

  // The good news inside the defect, and the reason this was a panel fix and not a builder one: the
  // control was inert, not lying about pixels. A phantom second axis would have been a rendering
  // bug and a different PR.
  it("emits a single y-axis, never the pair a dual axis needs", () => {
    expect(Array.isArray(build({ series: "g", rightAxisMetrics: ["w"] }).yAxis)).toBe(false);
    expect(Array.isArray(build({ series: "g" }).yAxis)).toBe(false);
  });

  it("builds the identical option whichever way the automatic split is set", () => {
    const on = build({ series: "g" });
    const off = build({ series: "g", autoSplitY: false });
    expect(JSON.stringify(off)).toBe(JSON.stringify(on));
  });

  /**
   * THE NEGATIVE, and the one that makes the three above mean something. Without a breakout the
   * same rows and the same config DO produce two axes and DO honour the switch — so the assertions
   * above are about `series`, not about a builder that never splits anything. Without this a gate
   * of `false` would satisfy all three.
   */
  it("still splits, and still obeys the switch, with no series", () => {
    const split = build({ rightAxisMetrics: ["w"] });
    expect(Array.isArray(split.yAxis)).toBe(true);
    expect(split.series.find((s) => s.name === "w")?.yAxisIndex).toBe(1);
    expect(Array.isArray(build({}).yAxis)).toBe(true);
    expect(Array.isArray(build({ autoSplitY: false }).yAxis)).toBe(false);
  });
});
