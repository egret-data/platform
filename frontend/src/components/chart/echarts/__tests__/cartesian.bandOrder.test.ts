import { describe, it, expect } from "vitest";
import { buildCartesianOption, cartesianSeriesNames } from "../options/cartesian";

/**
 * THE BAND ORDER A BREAKOUT PRODUCES, which was arrival order and therefore whatever the engine
 * happened to return. Measured on the demo dataset: the same card drew
 * `Doohickey / Gadget / Gizmo / Widget` on one page load and `Gizmo / Widget / Gadget / Doohickey`
 * on the next. Band order is the LEGEND order, the STACKING order and the DRAW order, so the two
 * loads are two different pictures of the same numbers.
 *
 * HOW THESE TESTS FAIL RELIABLY, which a nondeterminism test otherwise does not. Against the
 * unfixed builder an assertion that the output is sorted passes about half the time — whichever
 * order the fixture happens to be written in. Every fixture below therefore feeds the rows in a
 * DELIBERATELY WRONG order (descending, or interleaved), so `distinct`'s arrival order is exactly
 * the reverse of the asserted one and the unfixed code fails every run rather than by luck.
 *
 * The sibling of `sortCategories`' own tests: same defect, same fix, the other axis of the chart.
 */

const cols = [
  { name: "cat", type: "varchar" },
  { name: "band", type: "varchar" },
  { name: "rev", type: "double" },
];

/** Bands arriving in the WORST order for the assertion below: reverse-alphabetical. */
const rows = [
  ["x", "Widget", 4],
  ["x", "Gizmo", 3],
  ["x", "Gadget", 2],
  ["x", "Doohickey", 1],
];

const base = { kind: "bar", xAxis: "cat", yMetrics: ["rev"], series: "band" };
const build = (cfg: Record<string, unknown>, c = cols, r: unknown[][] = rows) =>
  buildCartesianOption(c, r, cfg as never) as unknown as {
    series: { name: string; __raw?: string; itemStyle?: { color?: string } }[];
  };

describe("breakout band order", () => {
  it("draws the bands in ascending order of the raw breakout value, not arrival order", () => {
    expect(build(base).series.map((s) => s.name)).toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
  });

  it("orders a numeric breakout numerically, not by its string form", () => {
    const c = [{ name: "cat", type: "varchar" }, { name: "band", type: "bigint" }, { name: "rev", type: "double" }];
    // 2 arrives last and sorts second: a string compare would put "10" before "2".
    const r = [["x", 10, 1], ["x", 1, 2], ["x", 2, 3]];
    expect(build(base, c, r).series.map((s) => s.name)).toEqual(["1", "2", "10"]);
  });

  it("orders a temporal breakout chronologically", () => {
    const c = [{ name: "cat", type: "varchar" }, { name: "band", type: "date" }, { name: "rev", type: "double" }];
    const r = [["x", "2026-03-01", 1], ["x", "2026-01-01", 2], ["x", "2026-02-01", 3]];
    expect(build(base, c, r).series.map((s) => s.__raw)).toEqual(["2026-01-01", "2026-02-01", "2026-03-01"]);
  });

  it("sorts on the RAW value, so a renamed band keeps its raw sort position", () => {
    // `seriesLabels` moves the DISPLAY name only. Sorting on the label would put "AAA" first;
    // sorting on the raw value leaves Widget last, where its raw value belongs.
    const o = build({ ...base, seriesLabels: { Widget: "AAA" } });
    expect(o.series.map((s) => s.name)).toEqual(["Doohickey", "Gadget", "Gizmo", "AAA"]);
    expect(o.series.map((s) => s.__raw)).toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
  });

  it("loses to an explicit seriesOrder", () => {
    const o = build({ ...base, seriesOrder: ["Widget", "Gizmo"] });
    expect(o.series.map((s) => s.name)).toEqual(["Widget", "Gizmo", "Doohickey", "Gadget"]);
  });

  it("does not fight hiddenSeries: what survives is still sorted", () => {
    const o = build({ ...base, hiddenSeries: ["Gizmo"] });
    expect(o.series.map((s) => s.name)).toEqual(["Doohickey", "Gadget", "Widget"]);
  });

  it("keeps arrival order when the query sorted BY the breakout column", () => {
    // The author asked for `band DESC` as the primary key, the engine returned the rows in it, and
    // re-sorting here would leave that sort visibly inert. Same bargain `preserveCategoryOrder`
    // strikes on the category axis.
    const o = build({ ...base, preserveSeriesOrder: true });
    expect(o.series.map((s) => s.name)).toEqual(["Widget", "Gizmo", "Gadget", "Doohickey"]);
  });

  it("still sorts the categories when only the band order is preserved", () => {
    const r = [["z", "Widget", 1], ["a", "Widget", 2]];
    const o = buildCartesianOption(cols, r, { ...base, preserveSeriesOrder: true } as never) as unknown as {
      xAxis: { data: string[] };
    };
    expect(o.xAxis.data).toEqual(["a", "z"]);
  });

  it("hands the settings panel the same order the chart draws", () => {
    // The panel's list IS the order the reorder arrows write back as `seriesOrder`. A panel listing
    // the bands in arrival order beside a chart drawing them sorted makes every arrow look broken.
    expect(cartesianSeriesNames(cols, rows, { yMetrics: ["rev"], xAxis: "cat", series: "band" }))
      .toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
  });

  it("sorts a scatter's breakout bands too", () => {
    const c = [
      { name: "band", type: "varchar" },
      { name: "spend", type: "double" },
      { name: "rev", type: "double" },
    ];
    const r = [["Widget", 1, 2], ["Gizmo", 3, 4], ["Doohickey", 5, 6]];
    const o = buildCartesianOption(c, r, {
      kind: "scatter", xMetric: "spend", yMetrics: ["rev"], series: "band",
    } as never) as unknown as { series: { name: string }[] };
    expect(o.series.map((s) => s.name)).toEqual(["Doohickey", "Gizmo", "Widget"]);
  });

  it("makes the STACKING order deterministic, not just the legend", () => {
    // The stack is drawn bottom-up in series order, so an arrival-ordered stack redraws itself
    // differently between two loads of the same card.
    const o = build({ ...base, stacked: true });
    expect(o.series.map((s) => s.name)).toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
  });

  it("gives a band the same colour whichever order its rows arrive in", () => {
    const forward = buildCartesianOption(cols, [...rows].reverse(), base as never) as unknown as {
      series: { name: string; itemStyle?: { color?: string } }[];
    };
    const backward = build(base);
    expect(forward.series.map((s) => [s.name, s.itemStyle?.color]))
      .toEqual(backward.series.map((s) => [s.name, s.itemStyle?.color]));
  });
});
